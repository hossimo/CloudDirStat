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
    End(&'a [String], &'a str, bool),
}

/// The elements open at the moment. Their names and texts keep their buffers when an
/// element closes, so a page of thousands of blobs allocates little beyond what the
/// caller keeps.
#[derive(Default)]
struct OpenElements {
    names: Vec<String>,
    texts: Vec<(String, bool)>,
    depth: usize,
}

impl OpenElements {
    fn open(&mut self, name: &[u8], encoded: bool) {
        let name = String::from_utf8_lossy(name);
        if self.depth < self.names.len() {
            self.names[self.depth].clear();
            self.names[self.depth].push_str(&name);
            let (text, text_encoded) = &mut self.texts[self.depth];
            text.clear();
            *text_encoded = encoded;
        } else {
            self.names.push(name.into_owned());
            self.texts.push((String::new(), encoded));
        }
        self.depth += 1;
    }

    fn path(&self) -> &[String] {
        &self.names[..self.depth]
    }

    /// The innermost element's text, to append to; `None` outside the root element.
    fn text(&mut self) -> Option<&mut String> {
        let depth = self.depth.checked_sub(1)?;
        Some(&mut self.texts[depth].0)
    }

    /// Reports the innermost element's end and closes it.
    fn close(&mut self, on: &mut impl FnMut(XmlEvent<'_>)) {
        let Some(depth) = self.depth.checked_sub(1) else {
            return;
        };
        let (text, encoded) = &self.texts[depth];
        on(XmlEvent::End(&self.names[..=depth], text, *encoded));
        self.depth = depth;
    }
}

pub(crate) fn walk(xml: &str, mut on: impl FnMut(XmlEvent<'_>)) -> Result<(), String> {
    let mut reader = Reader::from_str(xml);
    let mut open = OpenElements::default();
    loop {
        match reader.read_event().map_err(|error| error.to_string())? {
            Event::Start(element) => {
                let encoded = element
                    .try_get_attribute("Encoded")
                    .map_err(|error| error.to_string())?
                    .is_some_and(|attribute| attribute.value.as_ref() == b"true");
                open.open(element.local_name().as_ref(), encoded);
                on(XmlEvent::Start(open.path()));
            }
            Event::Empty(element) => {
                open.open(element.local_name().as_ref(), false);
                on(XmlEvent::Start(open.path()));
                open.close(&mut on);
            }
            Event::Text(text) => {
                if let Some(current) = open.text() {
                    current.push_str(&text.decode().map_err(|error| error.to_string())?);
                }
            }
            Event::CData(text) => {
                if let Some(current) = open.text() {
                    current.push_str(&text.decode().map_err(|error| error.to_string())?);
                }
            }
            Event::GeneralRef(reference) => {
                if let Some(current) = open.text() {
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
            Event::End(_) => open.close(&mut on),
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
                ends.push((path.join("/"), text.to_owned(), encoded));
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
