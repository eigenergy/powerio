//! The DGS V5 ASCII table reader and the object index it fills.
//!
//! A DGS file is a sequence of class tables. Each table opens with a header
//! line `$$ClassName;attr(type[:width]);...` and continues with one row per
//! object, fields separated by semicolons. The type codes are `a` (string),
//! `i` (integer), `r` (real), and `p` (object reference). A vector attribute
//! is declared as `name:SIZEROW(i)` followed by one `name:k(type)` descriptor
//! per declared element; a matrix adds `name:SIZECOL(i)` and `name:i:j(type)`
//! cell descriptors. The row layout is fixed by the header: a vector row
//! carries its actual length and then the declared number of cells, and a
//! matrix row carries its actual row and column counts and then the declared
//! cells, so a shorter actual size leaves the trailing cells empty.
//!
//! The `$$General` table carries the format version; only `5.0` is read.
//! Lines opening with `*` are comments. Decimal commas are accepted once one
//! real fails to parse with a decimal point, matching the PowSybl reader.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::{Error, Result};

/// The format label in hard failures.
pub(crate) const FMT: &str = "PowerFactory DGS";

/// One attribute value as the file states it.
#[derive(Clone, Debug, PartialEq)]
pub enum DgsValue {
    Str(String),
    Int(i64),
    Real(f64),
    Ref(RefKey),
    StrVec(Vec<String>),
    IntVec(Vec<i64>),
    RealVec(Vec<f64>),
    RefVec(Vec<RefKey>),
    /// Row major cells of an `actual_rows` by `actual_cols` matrix.
    RealMatrix {
        rows: usize,
        cols: usize,
        data: Vec<f64>,
    },
}

/// An object reference: a numeric object id or a `##name` foreign key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefKey {
    Id(i64),
    ForeignKey(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScalarType {
    Str,
    Int,
    Real,
    Ref,
}

impl ScalarType {
    fn from_code(code: &str) -> Option<Self> {
        Some(match code.chars().next()? {
            'a' => Self::Str,
            'i' => Self::Int,
            'r' => Self::Real,
            'p' => Self::Ref,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
enum AttrShape {
    Scalar,
    /// Declared element count.
    Vector(usize),
    /// Declared column count; a row may state fewer rows than declared.
    Matrix(usize),
}

/// One logical attribute of a class table and where its cells sit in a row.
#[derive(Clone, Debug)]
struct AttrDesc {
    name: String,
    ty: ScalarType,
    shape: AttrShape,
    /// Index of the first row field this attribute occupies.
    field: usize,
}

/// One class table header, shared by the objects it describes.
#[derive(Debug)]
pub struct ClassHeader {
    pub class: String,
    attrs: Vec<AttrDesc>,
    by_name: HashMap<String, usize>,
    /// Row field index of the `ID` column.
    id_field: usize,
    /// Line number of the header line.
    pub line: usize,
}

impl ClassHeader {
    /// The attribute names this table declares, in header order.
    #[must_use]
    pub fn attribute_names(&self) -> impl Iterator<Item = &str> {
        self.attrs.iter().map(|attr| attr.name.as_str())
    }

    /// Whether the table declares `name`.
    #[must_use]
    pub fn declares(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }
}

/// One object row: its class header, id, source position, and values.
#[derive(Debug)]
pub struct DgsObject {
    pub id: i64,
    header: Arc<ClassHeader>,
    /// One based line number of the row.
    pub line: usize,
    /// Byte range of the row within the decoded text.
    pub byte_start: usize,
    pub byte_end: usize,
    values: Vec<Option<DgsValue>>,
}

impl DgsObject {
    #[must_use]
    pub fn class(&self) -> &str {
        &self.header.class
    }

    /// The value of `name` when the table declares it and the row states it.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&DgsValue> {
        let index = *self.header.by_name.get(name)?;
        self.values.get(index).and_then(Option::as_ref)
    }

    #[must_use]
    pub fn str(&self, name: &str) -> Option<&str> {
        match self.get(name)? {
            DgsValue::Str(value) => Some(value.as_str()),
            _ => None,
        }
    }

    /// An integer value, also accepting a real that holds an integral value.
    #[must_use]
    pub fn int(&self, name: &str) -> Option<i64> {
        match self.get(name)? {
            DgsValue::Int(value) => Some(*value),
            DgsValue::Real(value) if value.fract() == 0.0 && value.abs() < 9.0e15 => {
                #[allow(clippy::cast_possible_truncation)]
                Some(*value as i64)
            }
            _ => None,
        }
    }

    /// A real value, also accepting an integer.
    #[must_use]
    pub fn real(&self, name: &str) -> Option<f64> {
        match self.get(name)? {
            DgsValue::Real(value) => Some(*value),
            #[allow(clippy::cast_precision_loss)]
            DgsValue::Int(value) => Some(*value as f64),
            _ => None,
        }
    }

    #[must_use]
    pub fn reference(&self, name: &str) -> Option<&RefKey> {
        match self.get(name)? {
            DgsValue::Ref(key) => Some(key),
            _ => None,
        }
    }

    #[must_use]
    pub fn real_vec(&self, name: &str) -> Option<&[f64]> {
        match self.get(name)? {
            DgsValue::RealVec(values) => Some(values.as_slice()),
            _ => None,
        }
    }

    #[must_use]
    pub fn ref_vec(&self, name: &str) -> Option<&[RefKey]> {
        match self.get(name)? {
            DgsValue::RefVec(values) => Some(values.as_slice()),
            _ => None,
        }
    }

    /// A matrix as `(rows, cols, row major data)`.
    #[must_use]
    pub fn matrix(&self, name: &str) -> Option<(usize, usize, &[f64])> {
        match self.get(name)? {
            DgsValue::RealMatrix { rows, cols, data } => Some((*rows, *cols, data.as_slice())),
            _ => None,
        }
    }

    /// The `loc_name` attribute, or the object id spelled out when absent.
    #[must_use]
    pub fn name(&self) -> String {
        self.str("loc_name")
            .map_or_else(|| self.id.to_string(), str::to_owned)
    }

    /// Whether the table declares `name`, whether or not this row states it.
    #[must_use]
    pub fn declares(&self, name: &str) -> bool {
        self.header.declares(name)
    }

    /// Every stated attribute as `(name, value)`, in header order.
    pub fn attributes(&self) -> impl Iterator<Item = (&str, &DgsValue)> {
        self.header
            .attrs
            .iter()
            .zip(&self.values)
            .filter_map(|(attr, value)| value.as_ref().map(|value| (attr.name.as_str(), value)))
    }
}

/// The decoded file: every object, indexed by id, class, and parent.
#[derive(Debug, Default)]
pub struct DgsDocument {
    /// `$$General` rows as `(Descr, Val)`.
    pub general: Vec<(String, String)>,
    objects: Vec<DgsObject>,
    by_id: HashMap<i64, usize>,
    by_foreign_key: HashMap<String, usize>,
    by_class: BTreeMap<String, Vec<usize>>,
    children: HashMap<i64, Vec<usize>>,
    /// Class tables in header order, including tables with no rows.
    classes: Vec<Arc<ClassHeader>>,
}

impl DgsDocument {
    /// Decode `text`. A structural failure (an unreadable header, a bad
    /// number, an unsupported version) is a hard error naming the line.
    ///
    /// # Errors
    /// [`Error::FormatRead`] on malformed text.
    pub fn parse(text: &str) -> Result<Self> {
        let mut reader = Reader::default();
        let mut offset = 0usize;
        for (index, raw_line) in text.split_inclusive('\n').enumerate() {
            let line_no = index + 1;
            let line = raw_line.trim_end_matches(['\n', '\r']);
            let leading = line.len() - line.trim_start().len();
            let trimmed = line.trim();
            let byte_start = offset + leading;
            let byte_end = byte_start + trimmed.len();
            offset += raw_line.len();
            if trimmed.is_empty() || trimmed.starts_with('*') {
                continue;
            }
            if let Some(header) = trimmed.strip_prefix("$$") {
                reader.open_table(header, line_no)?;
            } else {
                reader.read_row(trimmed, line_no, byte_start, byte_end)?;
            }
        }
        if !reader.version_seen {
            return Err(malformed(
                0,
                "no `$$General` table states the DGS version; only DGS 5.0 ASCII is read",
            ));
        }
        Ok(reader.finish())
    }

    /// Every object in file order.
    #[must_use]
    pub fn objects(&self) -> &[DgsObject] {
        &self.objects
    }

    /// The objects of `class`, in file order.
    #[must_use]
    pub fn of_class(&self, class: &str) -> impl Iterator<Item = &DgsObject> {
        self.by_class
            .get(class)
            .into_iter()
            .flatten()
            .map(|&index| &self.objects[index])
    }

    /// Row count per class, in class name order.
    #[must_use]
    pub fn class_counts(&self) -> impl Iterator<Item = (&str, usize)> {
        self.by_class
            .iter()
            .map(|(class, rows)| (class.as_str(), rows.len()))
    }

    /// The class tables in header order, rows or not.
    #[must_use]
    pub fn class_headers(&self) -> impl Iterator<Item = &ClassHeader> {
        self.classes.iter().map(Arc::as_ref)
    }

    #[must_use]
    pub fn by_id(&self, id: i64) -> Option<&DgsObject> {
        self.by_id.get(&id).map(|&index| &self.objects[index])
    }

    /// Resolve a reference by id or by `##for_name` foreign key.
    #[must_use]
    pub fn resolve(&self, key: &RefKey) -> Option<&DgsObject> {
        match key {
            RefKey::Id(id) => self.by_id(*id),
            RefKey::ForeignKey(name) => self
                .by_foreign_key
                .get(name)
                .map(|&index| &self.objects[index]),
        }
    }

    /// The object `name` refers to on `object`, when stated and resolvable.
    #[must_use]
    pub fn referenced(&self, object: &DgsObject, name: &str) -> Option<&DgsObject> {
        self.resolve(object.reference(name)?)
    }

    /// The parent named by `fold_id`, when stated and resolvable.
    #[must_use]
    pub fn parent(&self, object: &DgsObject) -> Option<&DgsObject> {
        self.referenced(object, "fold_id")
    }

    /// The objects whose `fold_id` names `id`, in file order.
    #[must_use]
    pub fn children(&self, id: i64) -> impl Iterator<Item = &DgsObject> {
        self.children
            .get(&id)
            .into_iter()
            .flatten()
            .map(|&index| &self.objects[index])
    }

    /// The children of `id` that belong to `class`.
    #[must_use]
    pub fn children_of_class<'a>(
        &'a self,
        id: i64,
        class: &'a str,
    ) -> impl Iterator<Item = &'a DgsObject> + 'a {
        self.children(id).filter(move |child| child.class() == class)
    }

    /// The declared DGS version from the `$$General` table.
    #[must_use]
    pub fn version(&self) -> Option<&str> {
        self.general
            .iter()
            .find(|(descr, _)| descr == "Version")
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Default)]
struct Reader {
    general: Vec<(String, String)>,
    in_general: bool,
    version_seen: bool,
    current: Option<Arc<ClassHeader>>,
    objects: Vec<DgsObject>,
    classes: Vec<Arc<ClassHeader>>,
    decimal_comma: bool,
}

impl Reader {
    fn open_table(&mut self, header: &str, line_no: usize) -> Result<()> {
        let mut fields = header.split(';');
        let class = fields.next().unwrap_or_default().trim();
        if class == "General" {
            self.in_general = true;
            self.current = None;
            return Ok(());
        }
        self.in_general = false;
        if class.is_empty() {
            return Err(malformed(line_no, "a table header names no class"));
        }
        let descriptors = fields.map(str::trim).collect::<Vec<_>>();
        let header = parse_header(class, &descriptors, line_no)?;
        let header = Arc::new(header);
        self.classes.push(Arc::clone(&header));
        self.current = Some(header);
        Ok(())
    }

    fn read_row(&mut self, row: &str, line_no: usize, start: usize, end: usize) -> Result<()> {
        let fields = split_fields(row);
        if self.in_general {
            if fields.len() < 3 {
                return Err(malformed(line_no, "a `$$General` row needs three fields"));
            }
            let descr = fields[1].trim().to_owned();
            let value = fields[2].trim().to_owned();
            if descr == "Version" {
                if value != "5.0" {
                    return Err(malformed(
                        line_no,
                        &format!(
                            "DGS version {value} is not read; only DGS 5.0 ASCII is read. \
                             Export the project again with the DGS V5 export definitions"
                        ),
                    ));
                }
                self.version_seen = true;
            }
            self.general.push((descr, value));
            return Ok(());
        }
        let Some(header) = self.current.clone() else {
            return Err(malformed(line_no, "a data row precedes every table header"));
        };
        let id_text = fields.get(header.id_field).map_or("", |f| f.trim());
        let id = id_text.parse::<i64>().map_err(|_| {
            malformed(
                line_no,
                &format!("object id `{id_text}` in `{}` is not an integer", header.class),
            )
        })?;
        let mut values = Vec::with_capacity(header.attrs.len());
        for attr in &header.attrs {
            values.push(self.read_value(attr, &fields, line_no, &header.class)?);
        }
        self.objects.push(DgsObject {
            id,
            header,
            line: line_no,
            byte_start: start,
            byte_end: end,
            values,
        });
        Ok(())
    }

    fn read_value(
        &mut self,
        attr: &AttrDesc,
        fields: &[&str],
        line_no: usize,
        class: &str,
    ) -> Result<Option<DgsValue>> {
        let cell = |index: usize| fields.get(index).map(|f| f.trim()).filter(|f| !f.is_empty());
        match attr.shape {
            AttrShape::Scalar => {
                let Some(text) = cell(attr.field) else {
                    return Ok(None);
                };
                Ok(Some(self.scalar(attr, text, line_no, class)?))
            }
            AttrShape::Vector(declared) => {
                let Some(count) = cell(attr.field) else {
                    return Ok(None);
                };
                let count = parse_count(count, &attr.name, line_no)?;
                if count > declared {
                    return Err(malformed(
                        line_no,
                        &format!(
                            "vector `{}` states {count} elements but the header declares {declared}",
                            attr.name
                        ),
                    ));
                }
                if count == 0 {
                    return Ok(None);
                }
                let mut cells = Vec::with_capacity(count);
                for k in 0..count {
                    let Some(text) = cell(attr.field + 1 + k) else {
                        return Ok(None);
                    };
                    cells.push(self.scalar(attr, text, line_no, class)?);
                }
                Ok(Some(collect_vector(attr.ty, cells)))
            }
            AttrShape::Matrix(declared_cols) => {
                let (Some(rows), Some(cols)) = (cell(attr.field), cell(attr.field + 1)) else {
                    return Ok(None);
                };
                let rows = parse_count(rows, &attr.name, line_no)?;
                let cols = parse_count(cols, &attr.name, line_no)?;
                if rows == 0 || cols == 0 {
                    return Ok(None);
                }
                if cols != declared_cols {
                    return Err(malformed(
                        line_no,
                        &format!(
                            "matrix `{}` states {cols} columns but the header declares {declared_cols}",
                            attr.name
                        ),
                    ));
                }
                let mut data = Vec::with_capacity(rows * cols);
                for k in 0..rows * cols {
                    let Some(text) = cell(attr.field + 2 + k) else {
                        return Ok(None);
                    };
                    data.push(self.real(text, &attr.name, line_no, class)?);
                }
                Ok(Some(DgsValue::RealMatrix { rows, cols, data }))
            }
        }
    }

    fn scalar(
        &mut self,
        attr: &AttrDesc,
        text: &str,
        line_no: usize,
        class: &str,
    ) -> Result<DgsValue> {
        Ok(match attr.ty {
            ScalarType::Str => DgsValue::Str(text.to_owned()),
            ScalarType::Int => DgsValue::Int(self.int(text, &attr.name, line_no, class)?),
            ScalarType::Real => DgsValue::Real(self.real(text, &attr.name, line_no, class)?),
            ScalarType::Ref => DgsValue::Ref(parse_ref(text, &attr.name, line_no, class)?),
        })
    }

    fn real(&mut self, text: &str, name: &str, line_no: usize, class: &str) -> Result<f64> {
        if self.decimal_comma {
            if let Ok(value) = text.replace(',', ".").parse::<f64>() {
                return Ok(value);
            }
        } else if let Ok(value) = text.parse::<f64>() {
            return Ok(value);
        } else if let Ok(value) = text.replace(',', ".").parse::<f64>() {
            self.decimal_comma = true;
            return Ok(value);
        }
        Err(malformed(
            line_no,
            &format!("`{class}.{name}` value `{text}` is not a real number"),
        ))
    }

    fn int(&mut self, text: &str, name: &str, line_no: usize, class: &str) -> Result<i64> {
        if let Ok(value) = text.parse::<i64>() {
            return Ok(value);
        }
        // An integer column exported with a decimal spelling of an integral
        // value reads as that integer.
        let real = self.real(text, name, line_no, class)?;
        if real.fract() == 0.0 && real.abs() < 9.0e15 {
            #[allow(clippy::cast_possible_truncation)]
            return Ok(real as i64);
        }
        Err(malformed(
            line_no,
            &format!("`{class}.{name}` value `{text}` is not an integer"),
        ))
    }

    fn finish(self) -> DgsDocument {
        let mut document = DgsDocument {
            general: self.general,
            objects: self.objects,
            classes: self.classes,
            ..DgsDocument::default()
        };
        for (index, object) in document.objects.iter().enumerate() {
            document.by_id.entry(object.id).or_insert(index);
            document
                .by_class
                .entry(object.class().to_owned())
                .or_default()
                .push(index);
            if let Some(name) = object.str("for_name").filter(|name| !name.is_empty()) {
                document.by_foreign_key.entry(name.to_owned()).or_insert(index);
            }
            if let Some(RefKey::Id(parent)) = object.reference("fold_id") {
                document.children.entry(*parent).or_default().push(index);
            }
        }
        // Foreign key parents resolve after every foreign key is indexed.
        let foreign_parents = document
            .objects
            .iter()
            .enumerate()
            .filter_map(|(index, object)| match object.reference("fold_id") {
                Some(RefKey::ForeignKey(name)) => document
                    .by_foreign_key
                    .get(name)
                    .map(|&parent| (document.objects[parent].id, index)),
                _ => None,
            })
            .collect::<Vec<_>>();
        for (parent, index) in foreign_parents {
            document.children.entry(parent).or_default().push(index);
        }
        document
    }
}

fn parse_header(class: &str, descriptors: &[&str], line_no: usize) -> Result<ClassHeader> {
    let mut attrs = Vec::new();
    let mut id_field = None;
    let mut index = 0usize;
    // An empty descriptor (a trailing semicolon on the header line) declares
    // nothing but still occupies its field position.
    let parsed = descriptors
        .iter()
        .map(|descriptor| {
            if descriptor.is_empty() {
                Ok((String::new(), ScalarType::Str))
            } else {
                parse_descriptor(descriptor, class, line_no)
            }
        })
        .collect::<Result<Vec<_>>>()?;
    while index < parsed.len() {
        let (name, ty) = &parsed[index];
        let field = index;
        if name.is_empty() {
            index += 1;
            continue;
        }
        if *name == "ID" {
            id_field = Some(field);
            index += 1;
            continue;
        }
        if let Some(base) = name.strip_suffix(":SIZEROW") {
            let is_matrix = parsed
                .get(index + 1)
                .is_some_and(|(next, _)| next.strip_suffix(":SIZECOL") == Some(base));
            let mut cell_ty = if is_matrix {
                ScalarType::Real
            } else {
                ScalarType::Int
            };
            let mut declared_rows = 0usize;
            let mut declared_cols = 0usize;
            let mut next = index + if is_matrix { 2 } else { 1 };
            while let Some((cell_name, cell_type)) = parsed.get(next) {
                let Some(rest) = cell_name
                    .strip_prefix(base)
                    .and_then(|rest| rest.strip_prefix(':'))
                else {
                    break;
                };
                let mut parts = rest.split(':');
                let row = parts.next().and_then(|r| r.parse::<usize>().ok());
                let col = parts.next().and_then(|c| c.parse::<usize>().ok());
                match (is_matrix, row, col) {
                    (false, Some(row), None) => declared_rows = row + 1,
                    (true, Some(row), Some(col)) => {
                        declared_rows = row + 1;
                        declared_cols = col + 1;
                    }
                    _ => break,
                }
                cell_ty = *cell_type;
                next += 1;
            }
            if is_matrix && cell_ty != ScalarType::Real {
                return Err(malformed(
                    line_no,
                    &format!("matrix `{class}.{base}` declares non real cells"),
                ));
            }
            attrs.push(AttrDesc {
                name: base.to_owned(),
                ty: cell_ty,
                shape: if is_matrix {
                    AttrShape::Matrix(declared_cols)
                } else {
                    AttrShape::Vector(declared_rows)
                },
                field,
            });
            index = next;
            continue;
        }
        attrs.push(AttrDesc {
            name: name.clone(),
            ty: *ty,
            shape: AttrShape::Scalar,
            field,
        });
        index += 1;
    }
    let Some(id_field) = id_field else {
        return Err(malformed(
            line_no,
            &format!("table `{class}` declares no `ID` column; only DGS 5.0 ASCII is read"),
        ));
    };
    let by_name = attrs
        .iter()
        .enumerate()
        .map(|(index, attr)| (attr.name.clone(), index))
        .collect();
    Ok(ClassHeader {
        class: class.to_owned(),
        attrs,
        by_name,
        id_field,
        line: line_no,
    })
}

/// `name(type[:width])` into its name and scalar type.
fn parse_descriptor(descriptor: &str, class: &str, line_no: usize) -> Result<(String, ScalarType)> {
    let (name, rest) = descriptor.split_once('(').ok_or_else(|| {
        malformed(
            line_no,
            &format!("attribute descriptor `{descriptor}` in `{class}` has no type"),
        )
    })?;
    let code = rest.trim_end_matches(')');
    let ty = ScalarType::from_code(code).ok_or_else(|| {
        malformed(
            line_no,
            &format!("attribute descriptor `{descriptor}` in `{class}` has an unknown type code"),
        )
    })?;
    Ok((name.trim().to_owned(), ty))
}

fn parse_count(text: &str, name: &str, line_no: usize) -> Result<usize> {
    text.parse::<usize>().map_err(|_| {
        malformed(
            line_no,
            &format!("`{name}` size `{text}` is not a nonnegative integer"),
        )
    })
}

fn parse_ref(text: &str, name: &str, line_no: usize, class: &str) -> Result<RefKey> {
    if let Some(foreign) = text.strip_prefix("##") {
        return Ok(RefKey::ForeignKey(foreign.to_owned()));
    }
    text.parse::<i64>().map(RefKey::Id).map_err(|_| {
        malformed(
            line_no,
            &format!("`{class}.{name}` reference `{text}` is neither an object id nor a `##` foreign key"),
        )
    })
}

fn collect_vector(ty: ScalarType, cells: Vec<DgsValue>) -> DgsValue {
    match ty {
        ScalarType::Str => DgsValue::StrVec(
            cells
                .into_iter()
                .filter_map(|cell| match cell {
                    DgsValue::Str(value) => Some(value),
                    _ => None,
                })
                .collect(),
        ),
        ScalarType::Int => DgsValue::IntVec(
            cells
                .into_iter()
                .filter_map(|cell| match cell {
                    DgsValue::Int(value) => Some(value),
                    _ => None,
                })
                .collect(),
        ),
        ScalarType::Real => DgsValue::RealVec(
            cells
                .into_iter()
                .filter_map(|cell| match cell {
                    DgsValue::Real(value) => Some(value),
                    _ => None,
                })
                .collect(),
        ),
        ScalarType::Ref => DgsValue::RefVec(
            cells
                .into_iter()
                .filter_map(|cell| match cell {
                    DgsValue::Ref(value) => Some(value),
                    _ => None,
                })
                .collect(),
        ),
    }
}

/// Split a row on `;`, keeping a double quoted span as one field that may
/// contain semicolons. Quotes are not escaped: a `"` opens a span and the
/// next `"` closes it.
fn split_fields(row: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut in_quotes = false;
    for (index, byte) in row.bytes().enumerate() {
        match byte {
            b'"' => in_quotes = !in_quotes,
            b';' if !in_quotes => {
                out.push(unquote(&row[start..index]));
                start = index + 1;
            }
            _ => {}
        }
    }
    out.push(unquote(&row[start..]));
    out
}

fn unquote(field: &str) -> &str {
    let trimmed = field.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(trimmed)
}

pub(crate) fn malformed(line_no: usize, message: &str) -> Error {
    Error::FormatRead {
        format: FMT,
        message: if line_no == 0 {
            message.to_owned()
        } else {
            format!("line {line_no}: {message}")
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "$$General;ID(a:40);Descr(a:40);Val(a:40)\n1;Version;5.0\n";

    #[test]
    fn scalars_vectors_and_matrices_follow_the_header_layout() {
        let text = format!(
            "{HEADER}\
$$ElmLne;ID(a:40);loc_name(a:40);fold_id(p);GPScoords:SIZEROW(i);GPScoords:SIZECOL(i);GPScoords:0:0(r);GPScoords:0:1(r);GPScoords:1:0(r);GPScoords:1:1(r);nlnum(i);inAir(i)
  25;\"Line;A\";2;2;2;45.0;9.0;45.1;9.1;3;0
  26;Line_B;2;0;0;;;;;1;0
$$ElmTow;ID(a:40);loc_name(a:40);plines:SIZEROW(i);plines:0(p);plines:1(p);dpolar:SIZEROW(i);dpolar:0(r);dpolar:1(r)
  12;tow;2;25;26;2;0;0,5
"
        );
        let document = DgsDocument::parse(&text).unwrap();
        let line = document.by_id(25).unwrap();
        assert_eq!(line.class(), "ElmLne");
        assert_eq!(line.str("loc_name"), Some("Line;A"));
        assert_eq!(line.reference("fold_id"), Some(&RefKey::Id(2)));
        assert_eq!(
            line.matrix("GPScoords"),
            Some((2, 2, [45.0, 9.0, 45.1, 9.1].as_slice()))
        );
        assert_eq!(line.int("nlnum"), Some(3));
        let other = document.by_id(26).unwrap();
        assert_eq!(other.matrix("GPScoords"), None);
        assert_eq!(other.int("nlnum"), Some(1));
        let tower = document.by_id(12).unwrap();
        assert_eq!(
            tower.ref_vec("plines"),
            Some([RefKey::Id(25), RefKey::Id(26)].as_slice())
        );
        assert_eq!(tower.real_vec("dpolar"), Some([0.0, 0.5].as_slice()));
        assert_eq!(document.of_class("ElmLne").count(), 2);
        assert_eq!(document.version(), Some("5.0"));
    }

    #[test]
    fn decimal_commas_are_accepted_after_the_first_failure() {
        let text = format!("{HEADER}$$TypLne;ID(a:40);rline(r);xline(r)\n7;0,5;1,25\n");
        let document = DgsDocument::parse(&text).unwrap();
        let typ = document.by_id(7).unwrap();
        assert_eq!(typ.real("rline"), Some(0.5));
        assert_eq!(typ.real("xline"), Some(1.25));
    }

    #[test]
    fn foreign_keys_resolve_references_and_parents() {
        let text = format!(
            "{HEADER}$$ElmNet;ID(a:40);loc_name(a:40);for_name(a:50)\n2;Net;net_a\n\
             $$ElmTerm;ID(a:40);loc_name(a:40);fold_id(p)\n3;Bus;##net_a\n"
        );
        let document = DgsDocument::parse(&text).unwrap();
        let bus = document.by_id(3).unwrap();
        assert_eq!(document.parent(bus).map(|net| net.id), Some(2));
        assert_eq!(document.children(2).count(), 1);
    }

    #[test]
    fn other_versions_and_missing_headers_are_refused() {
        let text = "$$General;ID(a:40);Descr(a:40);Val(a:40)\n1;Version;7.0\n";
        let error = DgsDocument::parse(text).unwrap_err().to_string();
        assert!(error.contains("7.0"), "{error}");
        let text = "$$ElmTerm;ID(a:40);loc_name(a:40)\n3;Bus\n";
        let error = DgsDocument::parse(text).unwrap_err().to_string();
        assert!(error.contains("version"), "{error}");
        let text = format!("{HEADER}$$ElmTerm;loc_name(a:40)\nBus\n");
        let error = DgsDocument::parse(&text).unwrap_err().to_string();
        assert!(error.contains("`ID`"), "{error}");
    }

    #[test]
    fn bad_numbers_name_the_line() {
        let text = format!("{HEADER}$$TypLne;ID(a:40);rline(r)\n7;abc\n");
        let error = DgsDocument::parse(&text).unwrap_err().to_string();
        assert!(error.contains("line 4"), "{error}");
        assert!(error.contains("TypLne.rline"), "{error}");
    }

    #[test]
    fn rows_record_their_byte_spans() {
        let text = format!("{HEADER}$$ElmTerm;ID(a:40);loc_name(a:40)\n  3;Bus\n");
        let document = DgsDocument::parse(&text).unwrap();
        let bus = document.by_id(3).unwrap();
        assert_eq!(&text[bus.byte_start..bus.byte_end], "3;Bus");
        assert_eq!(bus.line, 4);
    }
}
