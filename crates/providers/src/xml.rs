//! Just enough XML reading for REST responses (Azure's): walks the elements and hands
//! out each one's text, with entities resolved.

use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::Event;
use quick_xml::reader::Reader;

pub(crate) enum XmlEvent<'a> {
    /// An element starts; the path runs from the root element to it.
    Start(&'a [String]),
    /// An element ends, with its text and whether it has the attribute
    /// `Encoded="true"` (Azure percent-encodes blob names that XML cannot hold).
    End(&'a [String], String, bool),
}

pub(crate) fn walk(xml: &str, mut on: impl FnMut(XmlEvent<'_>)) -> Result<(), String> {
    let mut reader = Reader::from_str(xml);
    let mut path: Vec<String> = Vec::new();
    let mut texts: Vec<(String, bool)> = Vec::new();
    loop {
        match reader.read_event().map_err(|error| error.to_string())? {
            Event::Start(element) => {
                let encoded = element
                    .try_get_attribute("Encoded")
                    .map_err(|error| error.to_string())?
                    .is_some_and(|attribute| attribute.value.as_ref() == b"true");
                path.push(String::from_utf8_lossy(element.local_name().as_ref()).into_owned());
                texts.push((String::new(), encoded));
                on(XmlEvent::Start(&path));
            }
            Event::Empty(element) => {
                path.push(String::from_utf8_lossy(element.local_name().as_ref()).into_owned());
                on(XmlEvent::Start(&path));
                on(XmlEvent::End(&path, String::new(), false));
                path.pop();
            }
            Event::Text(text) => {
                if let Some((current, _)) = texts.last_mut() {
                    current.push_str(&text.decode().map_err(|error| error.to_string())?);
                }
            }
            Event::CData(text) => {
                if let Some((current, _)) = texts.last_mut() {
                    current.push_str(&text.decode().map_err(|error| error.to_string())?);
                }
            }
            Event::GeneralRef(reference) => {
                if let Some((current, _)) = texts.last_mut() {
                    match reference
                        .resolve_char_ref()
                        .map_err(|error| error.to_string())?
                    {
                        Some(character) => current.push(character),
                        None => {
                            let name = reference.decode().map_err(|error| error.to_string())?;
                            current.push_str(resolve_predefined_entity(&name).unwrap_or(""));
                        }
                    }
                }
            }
            Event::End(_) => {
                let (text, encoded) = texts.pop().unwrap_or_default();
                on(XmlEvent::End(&path, text, encoded));
                path.pop();
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hands_out_text_with_entities_resolved() {
        let xml =
            r#"<?xml version="1.0"?><A><B>x &amp; y&#33;</B><C Encoded="true">a%2Fb</C><D/></A>"#;
        let mut ends = Vec::new();
        walk(xml, |event| {
            if let XmlEvent::End(path, text, encoded) = event {
                ends.push((path.join("/"), text, encoded));
            }
        })
        .unwrap();
        assert_eq!(
            ends,
            [
                ("A/B".to_owned(), "x & y!".to_owned(), false),
                ("A/C".to_owned(), "a%2Fb".to_owned(), true),
                ("A/D".to_owned(), String::new(), false),
                ("A".to_owned(), String::new(), false),
            ]
        );
    }
}
