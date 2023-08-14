use std::{
    io::{self, Write},
    ops::Range,
    str,
};

const EMPTY_RANGE: Range<usize> = 0..0;
pub const EMPTY_RANGE_INDEX: usize = 0;
const ROOT_NODE_INDEX: usize = 0;
const INVALID_NODE_ID: usize = usize::MAX;
pub const ROOT_NODE_ID: usize = 0;

#[derive(Clone)]
struct TextNode {
    id: usize,
    range: Range<usize>,
}

#[derive(Copy, Clone)]
struct ElementNode {
    id: usize,
    name: usize,
    attrs: usize,
    kids: usize,
}

#[derive(Clone)]
enum Node {
    Text(TextNode),
    Element(ElementNode),
}

impl Node {
    fn id(&self) -> usize {
        match self {
            Node::Text(node) => node.id,
            Node::Element(node) => node.id,
        }
    }

    fn is_valid(&self) -> bool {
        match self {
            Node::Text(node) => node.id != INVALID_NODE_ID,
            Node::Element(node) => node.id != INVALID_NODE_ID,
        }
    }

    fn invalidate(&mut self) {
        match self {
            Node::Text(node) => node.id = INVALID_NODE_ID,
            Node::Element(node) => node.id = INVALID_NODE_ID,
        }
    }
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
    node_id_counter: usize,

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
            id: ROOT_NODE_ID,
            name: EMPTY_RANGE_INDEX,
            attrs: EMPTY_RANGE_INDEX,
            kids: EMPTY_RANGE_INDEX,
        });

        Self {
            text: Soup::new(),
            ranges,
            node_buf: Vec::new(),
            nodes: vec![root],
            node_id_counter: 0,
            attr_buf: Vec::new(),
            attrs: Vec::new(),
        }
    }

    fn get_node_by_id(&self, id: usize) -> Option<(usize, Node)> {
        self.nodes
            .iter()
            .enumerate()
            .find(|(_, node)| node.id() == id)
            .map(|(index, node)| (index, node.clone()))
    }

    pub fn get_node_id_by_index(&self, index: usize) -> Option<usize> {
        self.nodes.get(index).map(|node| node.id())
    }

    pub fn get_text_node_by_id(&self, id: usize) -> Option<TextNodeHandle> {
        if let Some((index, Node::Text(node))) = self.get_node_by_id(id) {
            return Some(TextNodeHandle {
                dom: self,
                index,
                node,
            });
        }
        None
    }

    pub fn get_text_node_mut(&mut self, id: usize) -> Option<TextNodeHandleMut> {
        if let Some((index, Node::Text(node))) = self.get_node_by_id(id) {
            return Some(TextNodeHandleMut {
                dom: self,
                index,
                node,
            });
        }
        None
    }

    pub fn get_element_node(&self, id: usize) -> Option<ElementNodeHandle> {
        if let Some((index, Node::Element(node))) = self.get_node_by_id(id) {
            return Some(ElementNodeHandle {
                dom: self,
                index,
                node,
            });
        }
        None
    }

    pub fn get_element_node_mut(&mut self, id: usize) -> Option<ElementNodeHandleMut> {
        if let Some((index, Node::Element(node))) = self.get_node_by_id(id) {
            return Some(ElementNodeHandleMut {
                dom: self,
                index,
                node,
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
            if !node.is_valid() {
                continue;
            }
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
            if !node.is_valid() {
                continue;
            }
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

    pub fn append_char(&mut self, c: char) {
        let mut buf = [0; 4];
        let string = c.encode_utf8(&mut buf);
        self.text.items.extend_from_slice(string.as_bytes())
    }

    pub fn get_text(&self, index: usize) -> Option<&str> {
        self.ranges
            .items
            .get(index)
            .map(|range| self.range_to_text(range.clone()))
    }

    fn range_to_text(&self, range: Range<usize>) -> &str {
        unsafe { str::from_utf8_unchecked(&self.text.items[range]) }
    }

    pub fn find_text(&self, text: &str) -> Option<usize> {
        if let Some(range) = self.text.find(text) {
            return self.ranges.find(&[range]).map(|range| range.start);
        }
        None
    }

    fn insert_range(&mut self, range: Range<usize>) -> usize {
        self.ranges.append(&[range]).start
    }

    pub fn write_tree(&self, writer: &mut dyn Write) -> io::Result<()> {
        let node = self.nodes[ROOT_NODE_INDEX].clone();
        self.write_node(0, node, writer)
    }

    pub fn write_junk(&self, writer: &mut dyn Write) -> io::Result<()> {
        for node in &self.nodes {
            if !node.is_valid() {
                self.write_node(0, node.clone(), writer)?;
            }
        }
        Ok(())
    }

    fn write_node(&self, depth: usize, node: Node, writer: &mut dyn Write) -> io::Result<()> {
        for _ in 0..depth {
            write!(writer, " ")?;
        }
        match node {
            Node::Text(node) => {
                let text = self.range_to_text(node.range);
                writeln!(writer, "<:{id}>{text}", id = node.id)?;
            }
            Node::Element(node) => {
                let name = self.get_text(node.name).unwrap();
                writeln!(writer, "<{name}:{id}>", id = node.id)?;
                let kids = self.ranges.items[node.kids].clone();
                for kid in &self.nodes[kids] {
                    self.write_node(depth + 2, kid.clone(), writer)?;
                }
            }
        }
        Ok(())
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
    pub fn set_text(&mut self, text: &str) -> Range<usize> {
        let range = self.dom.text.append(text);

        // update the node
        let node = TextNode {
            range: range.clone(),
            ..self.node
        };
        self.node = node.clone();
        self.dom.nodes[self.index] = Node::Text(node);

        range
    }
}

pub struct ElementNodeHandle<'a> {
    dom: &'a Dom,
    index: usize,
    node: ElementNode,
}

impl<'a> ElementNodeHandle<'a> {
    pub fn child_indicies(&self) -> Range<usize> {
        self.dom.ranges.items[self.node.kids].clone()
    }

    pub fn name(&self) -> usize {
        self.node.name
    }
}

pub struct ElementNodeHandleMut<'a> {
    dom: &'a mut Dom,
    index: usize,
    node: ElementNode,
}

impl<'a> ElementNodeHandleMut<'a> {
    pub fn children(&self) -> Range<usize> {
        self.dom.ranges.items[self.node.kids].clone()
    }

    /// returns id of appended node
    pub fn append_child_element(&mut self, name: usize, attrs: &[[usize; 2]]) -> usize {
        // sibling nodes *must* be contiguous in memory,
        // so we will copy the children into temp storage
        // TODO: sibling block freelist
        let kids = self.dom.ranges.items[self.node.kids].clone();
        for kid in &mut self.dom.nodes[kids] {
            self.dom.node_buf.push(kid.clone());
            // invalidate child
            kid.invalidate();
        }

        // add new child to temp storage
        for attr in attrs {
            self.dom.attr_buf.push(*attr);
        }
        let start = self.dom.attrs.len();
        let end = start + self.dom.attr_buf.len();
        self.dom.attrs.extend(self.dom.attr_buf.drain(..));
        let attrs = self.dom.insert_range(start..end);

        self.dom.node_id_counter += 1;
        self.dom.node_buf.push(Node::Element(ElementNode {
            id: self.dom.node_id_counter,
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
        let node = ElementNode { kids, ..self.node };
        self.node = node;
        self.dom.nodes[self.index] = Node::Element(node);

        self.dom.node_id_counter
    }

    /// returns id of appended node
    pub fn append_child_text(&mut self, text: &str) -> usize {
        // sibling nodes *must* be contiguous in memory,
        // so we will copy the children into temp storage
        // TODO: sibling block freelist
        let kids = self.dom.ranges.items[self.node.kids].clone();
        for kid in &mut self.dom.nodes[kids] {
            self.dom.node_buf.push(kid.clone());
            // invalidate child
            kid.invalidate();
        }

        // add new child to temp storage
        self.dom.node_id_counter += 1;
        let range = self.dom.text.append(text);
        self.dom.node_buf.push(Node::Text(TextNode {
            id: self.dom.node_id_counter,
            range: range.clone(),
        }));

        // copy all the childen back into nodes
        let start = self.dom.nodes.len();
        let end = start + self.dom.node_buf.len();
        self.dom.nodes.extend(self.dom.node_buf.drain(..));
        let kids = self.dom.insert_range(start..end);

        // update the parent node to new children
        let node = ElementNode { kids, ..self.node };
        self.node = node;
        self.dom.nodes[self.index] = Node::Element(node);

        self.dom.node_id_counter
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
        let node = ElementNode { attrs, ..self.node };
        self.node = node;
        self.dom.nodes[self.index] = Node::Element(node);

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

    fn find<I: AsRef<[T]>>(&self, items: I) -> Option<Range<usize>> {
        let items = items.as_ref();
        if let Some(start) = self
            .items
            .windows(items.len())
            .position(|window| window == items)
        {
            return Some(start..(start + items.len()));
        }
        None
    }

    fn append<I: AsRef<[T]>>(&mut self, items: I) -> Range<usize> {
        let items = items.as_ref();
        if let Some(range) = self.find(items) {
            return range;
        }
        // extra check, does the end of the soup partially match the start of the items?
        // TODO^ that
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
        let name = dom.insert_text("span");
        let id = dom.insert_text("id");
        let foo = dom.insert_text("foo");
        let index = dom
            .get_element_node_mut(ROOT_NODE_INDEX)
            .map(|mut root| root.append_child_element(name, &[[id, foo]]))
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
            dom.get_element_node_by_attr("id", "foo").unwrap().index
        );
    }
}
