//! A Live Set's XML, read as it streams out of its gzip, into a tree of
//! only what is wanted: a cover's set runs to hundreds of megabytes, most
//! of them automation, presets and binary aliases.

use std::collections::HashMap;
use std::io::BufRead;
use std::sync::Arc;

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

/// An element: its tag, its attributes and its children.
///
/// Tags and values are shared between all the elements that have the same:
/// a set repeats the same few thousand strings millions of times.
#[derive(Debug, Default)]
pub struct Node {
    pub tag: Arc<str>,
    pub attrs: Vec<(Arc<str>, Arc<str>)>,
    pub children: Vec<Node>,
}

impl Node {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| &**key == name)
            .map(|(_, value)| &**value)
    }

    /// The text inside it, which Live keeps binary blobs in, as hex.
    pub fn text(&self) -> Option<&str> {
        self.attr(TEXT)
    }

    /// The `Value` attribute, which Live keeps almost everything in.
    pub fn value(&self) -> Option<&str> {
        self.attr("Value")
    }

    pub fn child(&self, tag: &str) -> Option<&Node> {
        self.children.iter().find(|child| &*child.tag == tag)
    }

    pub fn children<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a Node> {
        self.children.iter().filter(move |child| &*child.tag == tag)
    }

    /// A descendant by its tags from here, `A/B/C`.
    pub fn at(&self, path: &str) -> Option<&Node> {
        path.split('/').try_fold(self, |node, tag| node.child(tag))
    }

    /// The `Value` of a descendant.
    pub fn value_at(&self, path: &str) -> Option<&str> {
        self.at(path)?.value()
    }

    pub fn parse_at<T: std::str::FromStr>(&self, path: &str) -> Option<T> {
        self.value_at(path)?.parse().ok()
    }

    /// Every descendant of a tag, outermost first, not looking inside one
    /// found.
    pub fn find_all<'a>(&'a self, tag: &str, into: &mut Vec<&'a Node>) {
        for child in &self.children {
            if &*child.tag == tag {
                into.push(child);
            } else {
                child.find_all(tag, into);
            }
        }
    }
}

/// Read the XML into a tree, leaving out every element `skip` says, by its
/// tag and its parent's, with all inside it.
pub fn parse(input: impl BufRead, skip: impl Fn(&str, &str) -> bool) -> Result<Node, String> {
    let mut reader = Reader::from_reader(input);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut strings = Strings::default();
    // The elements open, the document's root at the bottom.
    let mut open: Vec<Node> = vec![Node::default()];
    // How deep inside a skipped element the reader is.
    let mut skipping = 0usize;
    loop {
        let event = reader.read_event_into(&mut buffer).map_err(|e| {
            format!(
                "the set's XML is damaged at byte {}: {e}",
                reader.buffer_position()
            )
        })?;
        match event {
            Event::Start(start) => {
                if skipping > 0 {
                    skipping += 1;
                } else {
                    let node = node(&start, &mut strings)?;
                    let parent = open.last().map_or("", |parent| &*parent.tag);
                    if skip(&node.tag, parent) {
                        skipping = 1;
                    } else {
                        open.push(node);
                    }
                }
            }
            Event::Empty(empty) => {
                if skipping == 0 {
                    let node = node(&empty, &mut strings)?;
                    let parent = open.last().map_or("", |parent| &*parent.tag);
                    if !skip(&node.tag, parent) {
                        open.last_mut()
                            .expect("the root is never closed")
                            .children
                            .push(node);
                    }
                }
            }
            Event::End(_) => {
                if skipping > 0 {
                    skipping -= 1;
                } else if open.len() > 1 {
                    let mut node = open.pop().expect("checked above");
                    node.children.shrink_to_fit();
                    open.last_mut()
                        .expect("the root is never closed")
                        .children
                        .push(node);
                }
            }
            // Kept among the attributes, as few elements have any: binary
            // blobs, written out in hex.
            Event::Text(text) if skipping == 0 => {
                let text: &str = &text;
                if let Some(node) = open.last_mut() {
                    node.attrs.push((strings.get(TEXT), text.into()));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let mut root = open.swap_remove(0);
    match root.children.len() {
        1 => Ok(root.children.pop().expect("checked")),
        _ => Err("the set's XML has no single root".into()),
    }
}

/// What the text inside an element is kept as.
const TEXT: &str = "#text";

/// The strings met so far, each kept once.
#[derive(Default)]
struct Strings(HashMap<Arc<str>, ()>);

impl Strings {
    fn get(&mut self, text: &str) -> Arc<str> {
        if let Some((kept, ())) = self.0.get_key_value(text) {
            return kept.clone();
        }
        let kept: Arc<str> = text.into();
        self.0.insert(kept.clone(), ());
        kept
    }
}

fn node(start: &BytesStart, strings: &mut Strings) -> Result<Node, String> {
    let name = start.name();
    let tag = strings.get(<_ as AsRef<str>>::as_ref(&name));
    let mut attrs = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|e| e.to_string())?;
        let key = strings.get(<_ as AsRef<str>>::as_ref(&attribute.key));
        let value = attribute
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|e| e.to_string())?;
        attrs.push((key, strings.get(&value)));
    }
    Ok(Node {
        tag,
        attrs,
        children: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_without_what_is_skipped() {
        let xml = r#"<?xml version="1.0"?>
            <Ableton><LiveSet>
              <Name Value="A &amp; B" />
              <Big><Deep><Deeper Value="1"/></Deep></Big>
              <Kept Id="3"><Inner Value="x"/></Kept>
            </LiveSet></Ableton>"#;
        let root = parse(xml.as_bytes(), |tag, _| tag == "Big").unwrap();
        assert_eq!(&*root.tag, "Ableton");
        assert_eq!(root.value_at("LiveSet/Name"), Some("A & B"));
        assert!(root.at("LiveSet/Big").is_none());
        assert_eq!(root.at("LiveSet/Kept").unwrap().attr("Id"), Some("3"));
        assert_eq!(root.value_at("LiveSet/Kept/Inner"), Some("x"));
    }
}
