use std::collections::BTreeMap;

use powerio_core::{Error, SourceSpan};

use super::{GlmProject, Statement, Token, TokenKind};
use crate::diagnostics::codes;

const MAX_OBJECTS: usize = 1 << 20;
const MAX_PROPERTIES: usize = 1 << 20;

/// Index-local object identity. It is neither a native runtime ID nor the
/// explicit numeric key written after a class name in a GLM declaration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObjectKey(usize);

impl ObjectKey {
    #[must_use]
    pub const fn ordinal(self) -> usize {
        self.0
    }
}

/// A property retains ordered source tokens or an explicit inline object link.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum GlmPropertyValue<'a> {
    Tokens(&'a [Token]),
    InlineObject(ObjectKey),
}

#[derive(Clone, Debug)]
pub struct GlmProperty<'a> {
    pub name: &'a str,
    pub value: GlmPropertyValue<'a>,
    pub span: &'a SourceSpan,
}

/// One explicit declaration. Unknown classes/properties remain visible and are
/// not thereby certified as electrically supported. Properties are not cloned.
#[derive(Debug)]
pub struct GlmObject<'a> {
    key: ObjectKey,
    class_name: &'a str,
    explicit_id: Option<u32>,
    name: Option<String>,
    span: &'a SourceSpan,
    nested_parent: Option<ObjectKey>,
    properties: Vec<GlmProperty<'a>>,
    other_statements: Vec<&'a Statement>,
}

impl<'a> GlmObject<'a> {
    #[must_use]
    pub const fn key(&self) -> ObjectKey {
        self.key
    }
    #[must_use]
    pub const fn class_name(&self) -> &'a str {
        self.class_name
    }
    #[must_use]
    pub const fn explicit_id(&self) -> Option<u32> {
        self.explicit_id
    }
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
    #[must_use]
    pub const fn span(&self) -> &'a SourceSpan {
        self.span
    }
    #[must_use]
    pub fn properties(&self) -> &[GlmProperty<'a>] {
        &self.properties
    }
    /// Uninterpreted body syntax, never silently discarded by the index.
    #[must_use]
    pub fn other_statements(&self) -> &[&'a Statement] {
        &self.other_statements
    }
}

/// Borrowed explicit-object index. Building it resolves parents, rejects cycles
/// and ambiguous identities, and distinguishes nested from inline objects.
/// Equipment references are resolved on demand; no class schema or defaults
/// are inferred here. Dynamic objects, namespace blocks and relative/wildcard
/// references require further interpretation and are not silently approximated.
#[derive(Debug)]
pub struct ObjectIndex<'a> {
    project: &'a GlmProject,
    objects: Vec<GlmObject<'a>>,
    names: BTreeMap<String, ObjectKey>,
    ids: BTreeMap<u32, ObjectKey>,
    parents: Vec<Option<ObjectKey>>,
    property_count: usize,
}

impl GlmProject {
    pub fn index_objects(&self) -> Result<ObjectIndex<'_>, Error> {
        let mut index = ObjectIndex {
            project: self,
            objects: Vec::new(),
            names: BTreeMap::new(),
            ids: BTreeMap::new(),
            parents: Vec::new(),
            property_count: 0,
        };
        for statement in self.statements() {
            if let Statement::Block { header, body, span } = statement {
                match header.first().map(Token::text) {
                    Some("object") => {
                        index.declaration(header, body, span, None)?;
                    }
                    Some("namespace") => {
                        return Err(self.fail(
                            span,
                            "GLM namespaces require explicit qualification before object indexing",
                        ));
                    }
                    _ => {}
                }
            }
        }
        index.resolve_parents()?;
        Ok(index)
    }
}

impl<'a> ObjectIndex<'a> {
    pub(super) fn fail(&self, span: &SourceSpan, message: impl Into<String>) -> Error {
        self.project.fail(span, message)
    }

    #[must_use]
    pub fn objects(&self) -> &[GlmObject<'a>] {
        &self.objects
    }

    #[must_use]
    pub fn get(&self, key: ObjectKey) -> Option<&GlmObject<'a>> {
        self.objects.get(key.0)
    }

    #[must_use]
    pub fn parent(&self, key: ObjectKey) -> Option<ObjectKey> {
        self.parents.get(key.0).copied().flatten()
    }

    /// Resolve a name, `class:explicit_id`, or `id:explicit_id`. `root` is the
    /// native null reference. Anonymous objects do not acquire guessed aliases
    /// from their position in this index.
    pub fn resolve_reference(&self, reference: &str) -> Result<Option<ObjectKey>, Error> {
        if reference == "root" {
            return Ok(None);
        }
        let found = if let Some((class, numeric)) = reference.split_once(':') {
            let id = numeric.parse::<u32>().map_err(|_| {
                self.reference_error(reference, "unsupported symbolic or relative reference")
            })?;
            let key = self.ids.get(&id).copied().ok_or_else(|| {
                self.reference_error(reference, "explicit object ID does not exist")
            })?;
            let actual = self.objects[key.0]
                .class_name
                .rsplit('.')
                .next()
                .unwrap_or("");
            if class != "id" && class != actual {
                return Err(
                    self.reference_error(reference, "explicit object ID has a different class")
                );
            }
            Some(key)
        } else {
            self.names.get(reference).copied()
        };
        found
            .map(Some)
            .ok_or_else(|| self.reference_error(reference, "object name does not exist"))
    }

    /// Resolve an object-valued property. Repeated reference assignments are
    /// refused until their native deferred-assignment ordering is qualified.
    pub fn resolve_property(&self, key: ObjectKey, name: &str) -> Result<Option<ObjectKey>, Error> {
        let object = self
            .objects
            .get(key.0)
            .ok_or_else(|| self.reference_error(name, "owner key does not belong to this index"))?;
        let mut properties = object.properties.iter().filter(|p| p.name == name);
        let Some(property) = properties.next() else {
            return Ok(None);
        };
        if properties.next().is_some() {
            return Err(self.project.fail(property.span, format!("repeated object reference property {name} requires deferred-assignment ordering")));
        }
        match &property.value {
            GlmPropertyValue::InlineObject(key) => Ok(Some(*key)),
            GlmPropertyValue::Tokens(tokens) => {
                let reference =
                    literal(tokens).map_err(|message| self.project.fail(property.span, message))?;
                self.resolve_reference(&reference)
                    .map_err(|error| self.project.fail(property.span, error.to_string()))
            }
        }
    }

    fn reference_error(&self, reference: &str, message: &str) -> Error {
        Error::new(&codes::PARSE_GLM_OBJECT, format!("{message}: {reference}"))
            .with_source(self.project.source().clone())
    }

    fn declaration(
        &mut self,
        header: &'a [Token],
        body: &'a [Statement],
        span: &'a SourceSpan,
        parent: Option<ObjectKey>,
    ) -> Result<Option<ObjectKey>, Error> {
        if header.len() != 2 || header[0].text() != "object" || header[1].kind() != TokenKind::Word
        {
            return Err(self
                .project
                .fail(span, "expected object class or object class:numeric_id"));
        }
        let (class, first, count) =
            declaration_ids(header[1].text()).map_err(|m| self.project.fail(span, m))?;
        if count > MAX_OBJECTS - self.objects.len() {
            return Err(self
                .project
                .fail(span, "expanded GLM object count exceeds its limit"));
        }
        let mut last = None;
        for ordinal in 0..count {
            if self.objects.len() == MAX_OBJECTS {
                return Err(self
                    .project
                    .fail(span, "expanded GLM object count exceeds its limit"));
            }
            let id = first.map(|start| start + ordinal as u32);
            let key = ObjectKey(self.objects.len());
            if let Some(id) = id
                && self.ids.insert(id, key).is_some()
            {
                return Err(self.project.fail(
                    span,
                    "duplicate GLM explicit ID; numeric keys are global across classes",
                ));
            }
            self.objects.push(GlmObject {
                key,
                class_name: class,
                explicit_id: id,
                name: None,
                span,
                nested_parent: parent,
                properties: Vec::new(),
                other_statements: Vec::new(),
            });
            last = Some(key);
            self.body(key, body)?;
        }
        Ok(last)
    }

    fn body(&mut self, owner: ObjectKey, body: &'a [Statement]) -> Result<(), Error> {
        for statement in body {
            match statement {
                Statement::Simple { tokens, span }
                    if tokens.len() < 2
                        && tokens
                            .first()
                            .is_some_and(|t| matches!(t.text(), "name" | "parent" | "root")) =>
                {
                    return Err(self
                        .project
                        .fail(span, "GLM identity or parent property is missing its value"));
                }
                Statement::Simple { tokens, span } if tokens.len() >= 2 => {
                    let name = tokens[0].text();
                    if name == "name" {
                        let value =
                            literal(&tokens[1..]).map_err(|m| self.project.fail(span, m))?;
                        if self.objects[owner.0].name.is_some() || self.names.contains_key(&value) {
                            return Err(self
                                .project
                                .fail(span, "duplicate or reassigned GLM object name"));
                        }
                        self.names.insert(value.clone(), owner);
                        self.objects[owner.0].name = Some(value);
                    }
                    self.property(
                        owner,
                        GlmProperty {
                            name,
                            value: GlmPropertyValue::Tokens(&tokens[1..]),
                            span,
                        },
                    )?;
                }
                Statement::Block { header, body, span }
                    if header.first().is_some_and(|t| t.text() == "object") =>
                {
                    self.declaration(header, body, span, Some(owner))?;
                }
                Statement::Block { header, body, span }
                    if header.get(1).is_some_and(|t| t.text() == "object") =>
                {
                    let target = self
                        .declaration(&header[1..], body, span, None)?
                        .ok_or_else(|| self.project.fail(span, "inline object count is zero"))?;
                    self.property(
                        owner,
                        GlmProperty {
                            name: header[0].text(),
                            value: GlmPropertyValue::InlineObject(target),
                            span,
                        },
                    )?;
                }
                _ => self.objects[owner.0].other_statements.push(statement),
            }
        }
        Ok(())
    }

    fn property(&mut self, owner: ObjectKey, property: GlmProperty<'a>) -> Result<(), Error> {
        if self.property_count == MAX_PROPERTIES {
            return Err(self.project.fail(
                property.span,
                "expanded GLM property count exceeds its limit",
            ));
        }
        self.property_count += 1;
        self.objects[owner.0].properties.push(property);
        Ok(())
    }

    fn resolve_parents(&mut self) -> Result<(), Error> {
        for object in &self.objects {
            let explicit = object.properties.iter().any(|p| p.name == "parent");
            let root = object.properties.iter().any(|p| p.name == "root");
            if explicit && root {
                return Err(self.project.fail(
                    object.span,
                    "combined root/parent assignments require deferred-assignment ordering",
                ));
            }
            let parent = if explicit {
                self.resolve_property(object.key, "parent")?
            } else if root {
                None
            } else {
                object.nested_parent
            };
            self.parents.push(parent);
        }
        let mut state = vec![0u8; self.objects.len()];
        for start in 0..state.len() {
            if state[start] != 0 {
                continue;
            }
            let mut next = Some(ObjectKey(start));
            while let Some(key) = next {
                match state[key.0] {
                    2 => break,
                    1 => {
                        return Err(self.project.fail(
                            self.objects[key.0].span,
                            "GLM parent references form a cycle",
                        ));
                    }
                    _ => {
                        state[key.0] = 1;
                        next = self.parents[key.0];
                    }
                }
            }
            next = Some(ObjectKey(start));
            while let Some(key) = next {
                if state[key.0] == 2 {
                    break;
                }
                state[key.0] = 2;
                next = self.parents[key.0];
            }
        }
        Ok(())
    }
}

fn declaration_ids(text: &str) -> Result<(&str, Option<u32>, usize), &'static str> {
    let (class, ids) = text
        .split_once(':')
        .map_or((text, None), |(c, ids)| (c, Some(ids)));
    if class.is_empty()
        || !class
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.'))
    {
        return Err("invalid GLM class spelling");
    }
    let Some(ids) = ids else {
        return Ok((class, None, 1));
    };
    if let Some(count) = ids.strip_prefix("..") {
        let count = count
            .parse::<usize>()
            .map_err(|_| "invalid GLM object count")?;
        return Ok((class, None, count));
    }
    if let Some((first, last)) = ids.split_once("..") {
        let first = first
            .parse::<u32>()
            .map_err(|_| "invalid first GLM object ID")?;
        let last = last
            .parse::<u32>()
            .map_err(|_| "invalid last GLM object ID")?;
        let count = u64::from(last)
            .checked_sub(u64::from(first))
            .and_then(|n| usize::try_from(n + 1).ok())
            .ok_or("invalid or oversized GLM object range")?;
        return Ok((class, Some(first), count));
    }
    Ok((
        class,
        Some(ids.parse().map_err(|_| "invalid GLM object ID")?),
        1,
    ))
}

fn literal(tokens: &[Token]) -> Result<String, &'static str> {
    let [token] = tokens else {
        return Err("object names/references require one literal value");
    };
    let value = token.text();
    let value = if token.kind() == TokenKind::Quoted {
        value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .ok_or("object string literals require double quotes")?
    } else {
        value
    };
    if value.is_empty() || value.contains(['\\', '$', '\n', '\r']) {
        return Err("object names/references require a nonempty resolved literal without escapes");
    }
    Ok(value.to_owned())
}
