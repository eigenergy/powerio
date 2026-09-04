//! Reading rules shared by the XIIDM and CGMES readers on quick-xml.

use std::borrow::Cow;

use quick_xml::XmlVersion;
use quick_xml::encoding::Decoder;
use quick_xml::events::attributes::Attribute;

/// The XML version whose attribute value normalization applies.
///
/// XIIDM and CGMES documents are XML 1.0, so the readers apply the 1.0
/// rules without consulting the declaration: a literal tab, carriage
/// return, or line feed in an attribute value becomes one space, and a
/// character reference such as `&#10;` keeps its character.
const XML_VERSION: XmlVersion = XmlVersion::Implicit1_0;

/// The text of one attribute: transcoded to UTF-8, the predefined entities
/// expanded, and normalized under [`XML_VERSION`].
pub(crate) fn attribute_value<'a>(
    attribute: &Attribute<'a>,
    decoder: Decoder,
) -> quick_xml::Result<Cow<'a, str>> {
    attribute.decoded_and_normalized_value(XML_VERSION, decoder)
}

#[cfg(test)]
mod tests {
    use quick_xml::NsReader;
    use quick_xml::events::Event;

    #[test]
    fn attribute_values_follow_xml_1_0_normalization() {
        let mut reader = NsReader::from_str("<a name=\"x\ty\r\nz&#10;w&amp;v\"/>");
        let Ok((_, Event::Empty(element))) = reader.read_resolved_event() else {
            panic!("the element did not read as one empty element");
        };
        let attribute = element.attributes().next().unwrap().unwrap();
        let value = super::attribute_value(&attribute, reader.decoder()).unwrap();
        assert_eq!(value, "x y z\nw&v");
    }
}
