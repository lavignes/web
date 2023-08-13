use std::{ops::Range, str};

const EMPTY_RANGE: Range<usize> = 0..0;
const EMPTY_RANGE_INDEX: usize = 0;
const ROOT_NODE_INDEX: usize = 0;

#[derive(Copy, Clone)]
struct TextNode {
    range: usize,
}

#[derive(Copy, Clone)]
struct ElementNode {
    name: usize,
    attrs: usize,
    kids: usize,
}

#[derive(Copy, Clone)]
enum Node {
    Text(TextNode),
    Element(ElementNode),
}

// The basic theory of this dom is to pack data as tightly as possible
// data is always stored contiguously in memory so everything can be
// referred to by a range. An extra level of indirection is added however
// to also store ranges in memory so they can be interned just like text
pub struct Dom {
    text: Soup<u8>,
    ranges: Soup<Range<usize>>,

    node_buf: Vec<Node>, // temp working mem for moving nodes
    nodes: Vec<Node>,

    attr_buf: Vec<[usize; 2]>, // temp working mem for moving attrs
    attrs: Vec<[usize; 2]>,
}

impl Dom {
    pub fn new() -> Self {
        // range 0 is always the empty range
        let mut ranges = Soup::new();
        let empty_range_index = ranges.append(&[EMPTY_RANGE]).start;
        assert!(empty_range_index == EMPTY_RANGE_INDEX);

        // node 0 is the root node
        let root = Node::Element(ElementNode {
            name: EMPTY_RANGE_INDEX,
            attrs: EMPTY_RANGE_INDEX,
            kids: EMPTY_RANGE_INDEX,
        });

        Self {
            text: Soup::new(),
            ranges,
            node_buf: Vec::new(),
            nodes: vec![root],
            attr_buf: Vec::new(),
            attrs: Vec::new(),
        }
    }

    pub fn get_text_node(&self, index: usize) -> Option<TextNodeHandle> {
        if let Some(Node::Text(node)) = self.nodes.get(index) {
            return Some(TextNodeHandle {
                dom: self,
                index,
                node: *node,
            });
        }
        None
    }

    pub fn get_text_node_mut(&mut self, index: usize) -> Option<TextNodeHandleMut> {
        if let Some(Node::Text(node)) = self.nodes.get(index) {
            return Some(TextNodeHandleMut {
                node: *node,
                index,
                dom: self,
            });
        }
        None
    }

    pub fn get_element_node(&self, index: usize) -> Option<ElementNodeHandle> {
        if let Some(Node::Element(node)) = self.nodes.get(index) {
            return Some(ElementNodeHandle {
                dom: self,
                index,
                node: *node,
            });
        }
        None
    }

    pub fn get_element_node_mut(&mut self, index: usize) -> Option<ElementNodeHandleMut> {
        if let Some(Node::Element(node)) = self.nodes.get(index) {
            return Some(ElementNodeHandleMut {
                node: *node,
                dom: self,
                index,
            });
        }
        None
    }

    pub fn get_element_node_by_attr(
        &mut self,
        name: &str,
        value: &str,
    ) -> Option<ElementNodeHandle> {
        let name = self.insert_text(name);
        let value = self.insert_text(value);
        for (index, node) in self.nodes.iter().enumerate() {
            if let Node::Element(node) = node {
                let attrs = self.ranges.items[node.attrs].clone();
                for attr in &self.attrs[attrs] {
                    if (attr[0] == name) && (attr[1] == value) {
                        return Some(ElementNodeHandle {
                            dom: self,
                            index,
                            node: *node,
                        });
                    }
                }
            }
        }
        None
    }

    pub fn get_element_node_by_attr_mut(
        &mut self,
        name: &str,
        value: &str,
    ) -> Option<ElementNodeHandleMut> {
        let name = self.insert_text(name);
        let value = self.insert_text(value);
        for (index, node) in self.nodes.iter().enumerate() {
            if let Node::Element(node) = node {
                let attrs = self.ranges.items[node.attrs].clone();
                for attr in &self.attrs[attrs] {
                    if (attr[0] == name) && (attr[1] == value) {
                        return Some(ElementNodeHandleMut {
                            node: *node,
                            dom: self,
                            index,
                        });
                    }
                }
            }
        }
        None
    }

    pub fn insert_text(&mut self, text: &str) -> usize {
        let range = self.text.append(text);
        self.insert_range(range)
    }

    pub fn get_text(&mut self, index: usize) -> Option<&str> {
        self.ranges
            .items
            .get(index)
            .map(|range| unsafe { str::from_utf8_unchecked(&self.text.items[range.clone()]) })
    }

    fn insert_range(&mut self, range: Range<usize>) -> usize {
        self.ranges.append(&[range]).start
    }
}

pub struct TextNodeHandle<'a> {
    dom: &'a Dom,
    index: usize,
    node: TextNode,
}

pub struct TextNodeHandleMut<'a> {
    dom: &'a mut Dom,
    index: usize,
    node: TextNode,
}

impl<'a> TextNodeHandleMut<'a> {
    pub fn set_text(&mut self, value: &str) -> usize {
        let range = self.dom.insert_text(value);
        self.dom.node_buf.push(Node::Text(TextNode { range }));
        range
    }
}

pub struct ElementNodeHandle<'a> {
    dom: &'a Dom,
    index: usize,
    node: ElementNode,
}

pub struct ElementNodeHandleMut<'a> {
    dom: &'a mut Dom,
    index: usize,
    node: ElementNode,
}

impl<'a> ElementNodeHandleMut<'a> {
    /// returns index of appended node
    fn append_child_element(&mut self, name: &str, attrs: &[[&str; 2]]) -> usize {
        // sibling nodes *must* be contiguous in memory,
        // so we will copy the children into temp storage
        // TODO: sibling block freelist
        let kids = self.dom.ranges.items[self.node.kids].clone();
        self.dom.node_buf.extend_from_slice(&self.dom.nodes[kids]);

        // add new child to temp storage
        for attr in attrs {
            let name = self.dom.insert_text(attr[0]);
            let value = self.dom.insert_text(attr[1]);
            self.dom.attr_buf.push([name, value]);
        }
        let start = self.dom.attrs.len();
        let end = start + self.dom.attr_buf.len();
        self.dom.attrs.extend(self.dom.attr_buf.drain(..));
        let attrs = self.dom.insert_range(start..end);

        let name = self.dom.insert_text(name);
        self.dom.node_buf.push(Node::Element(ElementNode {
            name,
            attrs,
            kids: EMPTY_RANGE_INDEX,
        }));

        // copy all the childen back into nodes
        let start = self.dom.nodes.len();
        let end = start + self.dom.node_buf.len();
        self.dom.nodes.extend(self.dom.node_buf.drain(..));
        let kids = self.dom.insert_range(start..end);

        // update the parent node to new children
        self.dom.nodes[self.index] = Node::Element(ElementNode { kids, ..self.node });

        // since we are appending, the index of the child is the last node
        // TODO: at least until we add a freelist
        self.dom.nodes.len() - 1
    }

    /// returns possibly updated index of attrs for node
    // TODO: I don't even think I need to add attributes
    fn insert_attribute(&mut self, name: &str, value: &str) -> usize {
        // like sibling nodes, attrs *must* be contiguous in memory
        // we'll copy them into temp storage
        // TODO: attr block freelist
        let attrs = self.dom.ranges.items[self.node.attrs].clone();
        self.dom.attr_buf.extend_from_slice(&self.dom.attrs[attrs]);

        // add new attr (updating if it already exists)
        let name = self.dom.insert_text(name);
        let value = self.dom.insert_text(value);
        if let Some(attr) = self.dom.attr_buf.iter_mut().find(|[k, _]| *k == name) {
            attr[1] = value;
        } else {
            self.dom.attr_buf.push([name, value]);
        }

        // copy all the ranges back in
        let start = self.dom.attrs.len();
        let end = start + self.dom.attr_buf.len();
        self.dom.attrs.extend(self.dom.attr_buf.drain(..));
        let attrs = self.dom.insert_range(start..end);

        // update the node
        self.dom.nodes[self.index] = Node::Element(ElementNode { attrs, ..self.node });
        attrs
    }
}

struct Soup<T> {
    items: Vec<T>,
}

impl<T> Default for Soup<T> {
    fn default() -> Self {
        Self {
            items: Vec::default(),
        }
    }
}

impl<T: Eq + Clone> Soup<T> {
    fn new() -> Self {
        Self::default()
    }

    fn append<I: AsRef<[T]>>(&mut self, items: I) -> Range<usize> {
        let items = items.as_ref();
        if let Some(start) = self
            .items
            .windows(items.len())
            .position(|window| window == items)
        {
            return start..(start + items.len());
        }
        let start = self.items.len();
        self.items.extend_from_slice(items);
        start..(start + items.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soup_insert() {
        let mut soup = Soup::new();
        let range = soup.append("hello world");
        assert_eq!(0..11, range);
        let range = soup.append("or");
        assert_eq!("hello world".as_bytes(), soup.items);
        assert_eq!(7..9, range);
        let range = soup.append("test");
        assert_eq!("hello worldtest".as_bytes(), soup.items);
        assert_eq!(11..15, range);
    }

    #[test]
    fn root_attrs() {
        let mut dom = Dom::new();
        // add and update some attrs
        {
            let mut root = dom.get_element_node_mut(ROOT_NODE_INDEX).unwrap();
            let attrs = root.insert_attribute("key", "value");
            let attrs = root.dom.ranges.items[attrs].clone();
            let attr = root.dom.attrs[attrs][0].clone();
            assert_eq!("key", root.dom.get_text(attr[0]).unwrap());
            assert_eq!("value", root.dom.get_text(attr[1]).unwrap());

            let attrs = root.insert_attribute("key", "new");
            let attrs = root.dom.ranges.items[attrs].clone();
            let attr = root.dom.attrs[attrs][0].clone();
            assert_eq!("key", root.dom.get_text(attr[0]).unwrap());
            assert_eq!("new", root.dom.get_text(attr[1]).unwrap());
        }
        assert_eq!("keyvaluenew".as_bytes(), dom.text.items);

        // find root node by attr
        assert_eq!(
            ROOT_NODE_INDEX,
            dom.get_element_node_by_attr("key", "new").unwrap().index
        );
    }

    #[test]
    fn append_child() {
        let mut dom = Dom::new();
        let index = dom
            .get_element_node_mut(ROOT_NODE_INDEX)
            .map(|mut root| root.append_child_element("span", &[["id", "foo"]]))
            .unwrap();
        assert_eq!(1, index);

        {
            let mut child = dom.get_element_node_mut(index).unwrap();
            let attrs = child.insert_attribute("key", "value");
            let attrs = child.dom.ranges.items[attrs].clone();
            let attr = child.dom.attrs[attrs][1].clone();
            assert_eq!("key", child.dom.get_text(attr[0]).unwrap());
            assert_eq!("value", child.dom.get_text(attr[1]).unwrap());
        }

        assert_eq!(
            index,
            dom.get_element_node_by_attr("key", "value").unwrap().index
        );

        assert_eq!(
            index,
            dom.get_element_node_by_attr("key", "value").unwrap().index
        );
    }
}
