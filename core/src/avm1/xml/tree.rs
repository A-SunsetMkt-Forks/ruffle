//! XML Tree structure

use crate::avm1::xml::iterators;
use crate::avm1::{Activation, ArrayBuilder, Attribute, Error, NativeObject, Object, Value};
use crate::string::{AvmString, StringContext, WStr, WString};
use gc_arena::barrier::unlock;
use gc_arena::{
    lock::{Lock, RefLock},
    Collect, Gc, Mutation,
};
use quick_xml::escape::escape;
use quick_xml::events::BytesStart;
use ruffle_macros::istr;
use std::cell::RefMut;
use std::fmt;

pub const ELEMENT_NODE: u8 = 1;
pub const TEXT_NODE: u8 = 3;

/// Represents a node in the XML tree.
#[derive(Copy, Clone, Collect)]
#[collect(no_drop)]
pub struct XmlNode<'gc>(Gc<'gc, XmlNodeData<'gc>>);

#[derive(Clone, Collect)]
#[collect(no_drop)]
struct XmlNodeData<'gc> {
    /// The script object associated with this XML node, if any.
    script_object: Lock<Option<Object<'gc>>>,

    /// The parent node of this one.
    parent: Lock<Option<XmlNode<'gc>>>,

    /// The previous sibling node to this one.
    prev_sibling: Lock<Option<XmlNode<'gc>>>,

    /// The next sibling node to this one.
    next_sibling: Lock<Option<XmlNode<'gc>>>,

    /// The type of this XML node. Should either `ELEMENT_NODE` or `TEXT_NODE`,
    /// but any other value is accepted as well.
    node_type: u8,

    /// The tag name of this element, or its text content, depending on `node_type`.
    /// None if this is a document root node.
    node_value: Lock<Option<AvmString<'gc>>>,

    /// Attributes of the element.
    attributes: Object<'gc>,

    /// The array object used for AS2 `.childNodes`
    cached_child_nodes: Lock<Option<Object<'gc>>>,

    /// Child nodes of this element.
    children: RefLock<Vec<XmlNode<'gc>>>,
}

impl<'gc> XmlNode<'gc> {
    /// Construct a new XML node.
    pub fn new(mc: &Mutation<'gc>, node_type: u8, node_value: Option<AvmString<'gc>>) -> Self {
        Self(Gc::new(
            mc,
            XmlNodeData {
                script_object: Lock::new(None),
                parent: Lock::new(None),
                prev_sibling: Lock::new(None),
                next_sibling: Lock::new(None),
                node_type,
                node_value: Lock::new(node_value),
                attributes: Object::new_without_proto(mc),
                cached_child_nodes: Lock::new(None),
                children: RefLock::new(Vec::new()),
            },
        ))
    }

    /// Construct an XML Element node from a `quick_xml` `BytesStart` event.
    ///
    /// The returned node will always be an `Element`, and it must only contain
    /// valid encoded UTF-8 data. (Other encoding support is planned later.)
    pub fn from_start_event(
        activation: &mut Activation<'_, 'gc>,
        bs: BytesStart<'_>,
        id_map: Object<'gc>,
        decoder: quick_xml::Decoder,
    ) -> Result<Self, quick_xml::Error> {
        let name = AvmString::new_utf8_bytes(activation.gc(), bs.name().into_inner());
        let node = Self::new(activation.gc(), ELEMENT_NODE, Some(name));

        // Reverse attributes so they appear in the `PropertyMap` in their definition order.
        let attributes: Result<Vec<_>, _> = bs.attributes().collect();
        let attributes = attributes?;
        for attribute in attributes.iter().rev() {
            let key = AvmString::new_utf8_bytes(activation.gc(), attribute.key.into_inner());
            let value_str = crate::xml::custom_unescape(&attribute.value, decoder)?;
            let value = AvmString::new_utf8_bytes(activation.gc(), value_str.as_bytes());

            // Insert an attribute.
            node.attributes()
                .define_value(activation.gc(), key, value.into(), Attribute::empty());

            // Update the ID map.
            if attribute.key.into_inner() == b"id" {
                id_map.define_value(
                    activation.gc(),
                    value,
                    node.script_object(activation).into(),
                    Attribute::empty(),
                );
            }
        }

        Ok(node)
    }

    /// Get the parent, if this node has one.
    pub fn parent(self) -> Option<XmlNode<'gc>> {
        self.0.parent.get()
    }

    /// Set the parent of this node.
    fn set_parent(self, mc: &Mutation<'gc>, parent: Option<XmlNode<'gc>>) {
        unlock!(Gc::write(mc, self.0), XmlNodeData, parent).set(parent);
    }

    /// Get the previous sibling, if this node has one.
    pub fn prev_sibling(self) -> Option<XmlNode<'gc>> {
        self.0.prev_sibling.get()
    }

    /// Set this node's previous sibling.
    fn set_prev_sibling(self, mc: &Mutation<'gc>, new_prev: Option<XmlNode<'gc>>) {
        unlock!(Gc::write(mc, self.0), XmlNodeData, prev_sibling).set(new_prev);
    }

    /// Get the next sibling, if this node has one.
    pub fn next_sibling(self) -> Option<XmlNode<'gc>> {
        self.0.next_sibling.get()
    }

    /// Set this node's next sibling.
    fn set_next_sibling(self, mc: &Mutation<'gc>, new_next: Option<XmlNode<'gc>>) {
        unlock!(Gc::write(mc, self.0), XmlNodeData, next_sibling).set(new_next);
    }

    /// Remove node from its current siblings list.
    ///
    /// If a former sibling exists, we will also adopt it to the opposing side
    /// of this node, so as to maintain a coherent sibling list.
    ///
    /// This is the opposite of `adopt_siblings` - the former adds a node to a
    /// new sibling list, and this removes it from the current one.
    fn disown_siblings(self, mc: &Mutation<'gc>) {
        let old_prev = self.prev_sibling();
        let old_next = self.next_sibling();

        if let Some(prev) = old_prev {
            prev.set_next_sibling(mc, old_next);
        }

        if let Some(next) = old_next {
            next.set_prev_sibling(mc, old_prev);
        }

        self.set_prev_sibling(mc, None);
        self.set_next_sibling(mc, None);
    }

    /// Add node to a new siblings list.
    ///
    /// If a given sibling exists, we will also ensure this node is adopted as
    /// its sibling, so as to maintain a coherent sibling list.
    ///
    /// This is the opposite of `disown_siblings` - the former removes a
    /// sibling from its current list, and this adds the sibling to a new one.
    fn adopt_siblings(
        self,
        mc: &Mutation<'gc>,
        new_prev: Option<XmlNode<'gc>>,
        new_next: Option<XmlNode<'gc>>,
    ) {
        if let Some(prev) = new_prev {
            prev.set_next_sibling(mc, Some(self));
        }

        if let Some(next) = new_next {
            next.set_prev_sibling(mc, Some(self));
        }

        self.set_prev_sibling(mc, new_prev);
        self.set_next_sibling(mc, new_next);
    }

    /// Remove node from this node's child list.
    ///
    /// This function yields Err if this node cannot accept child nodes.
    fn orphan_child(self, mc: &Mutation<'gc>, child: XmlNode<'gc>) {
        if let Some(position) = self.child_position(child) {
            self.children_mut(mc).remove(position);
        }
    }

    /// Insert `child` into the children list of this node.
    ///
    /// `child` will be adopted into the current tree: all child references
    /// to other nodes or documents will be adjusted to reflect its new
    /// position in the tree. This may remove it from any existing trees or
    /// documents.
    ///
    /// The `position` parameter is the position of the new child in
    /// this node's children list. This is used to find and link the child's
    /// siblings to each other.
    pub fn insert_child(self, mc: &Mutation<'gc>, position: usize, child: XmlNode<'gc>) {
        let is_cyclic = self
            .ancestors()
            .any(|ancestor| Gc::ptr_eq(ancestor.0, child.0));
        if is_cyclic {
            return;
        }

        if let Some(old_parent) = child.parent() {
            if !Gc::ptr_eq(self.0, old_parent.0) {
                old_parent.orphan_child(mc, child);
            }
        }

        child.set_parent(mc, Some(self));

        let mut children = self.children_mut(mc);
        children.insert(position, child);

        let new_prev = position
            .checked_sub(1)
            .and_then(|p| children.get(p).cloned());
        let new_next = position
            .checked_add(1)
            .and_then(|p| children.get(p).cloned());
        child.adopt_siblings(mc, new_prev, new_next);
    }

    /// Append a child element into the end of the child list of an element node.
    pub fn append_child(self, mc: &Mutation<'gc>, child: XmlNode<'gc>) {
        self.insert_child(mc, self.children_len(), child);
    }

    /// Remove this node from its parent.
    pub fn remove_node(self, mc: &Mutation<'gc>) {
        if let Some(parent) = self.parent() {
            // This is guaranteed to succeed, as `self` is a child of `parent`.
            let position = parent.child_position(self).unwrap();
            parent.children_mut(mc).remove(position);

            self.disown_siblings(mc);
            self.set_parent(mc, None);
        }
    }

    /// Returns the type of this node as an integer.
    pub fn node_type(self) -> u8 {
        self.0.node_type
    }

    /// Returns the tag name of this element, if any.
    pub fn node_name(self) -> Option<AvmString<'gc>> {
        if self.0.node_type == ELEMENT_NODE {
            self.0.node_value.get()
        } else {
            None
        }
    }

    pub fn local_name(self, gc_context: &Mutation<'gc>) -> Option<AvmString<'gc>> {
        self.node_name().map(|name| match name.find(b':') {
            Some(i) if i + 1 < name.len() => AvmString::new(gc_context, &name[i + 1..]),
            _ => name,
        })
    }

    pub fn prefix(self, context: &mut StringContext<'gc>) -> Option<AvmString<'gc>> {
        self.node_name().map(|name| match name.find(b':') {
            Some(i) if i + 1 < name.len() => AvmString::new(context.gc(), &name[..i]),
            _ => context.empty(),
        })
    }

    /// Returns the node value of this node, if any.
    pub fn node_value(self) -> Option<AvmString<'gc>> {
        if self.0.node_type == ELEMENT_NODE {
            None
        } else {
            self.0.node_value.get()
        }
    }

    pub fn set_node_value(self, gc_context: &Mutation<'gc>, value: AvmString<'gc>) {
        unlock!(Gc::write(gc_context, self.0), XmlNodeData, node_value).set(Some(value));
    }

    /// Returns the number of children of the current tree node.
    pub fn children_len(self) -> usize {
        self.0.children.borrow().len()
    }

    /// Get the position of a child of this node.
    ///
    /// This function yields None if the node cannot accept children or if the
    /// child node is not a child of this node.
    pub fn child_position(self, child: XmlNode<'gc>) -> Option<usize> {
        self.children()
            .position(|other| Gc::ptr_eq(child.0, other.0))
    }

    /// Checks if `child` is a direct descendant of `self`.
    pub fn has_child(self, child: XmlNode<'gc>) -> bool {
        child.parent().filter(|p| Gc::ptr_eq(self.0, p.0)).is_some()
    }

    /// Retrieve a given child by index.
    pub fn get_child_by_index(self, index: usize) -> Option<XmlNode<'gc>> {
        self.0.children.borrow().get(index).copied()
    }

    /// Returns an iterator that yields child nodes.
    pub fn children(self) -> impl DoubleEndedIterator<Item = XmlNode<'gc>> {
        iterators::ChildIter::for_node(self)
    }

    /// Returns a mutable reference to child nodes.
    pub fn children_mut(&self, mc: &Mutation<'gc>) -> RefMut<'_, Vec<XmlNode<'gc>>> {
        unlock!(Gc::write(mc, self.0), XmlNodeData, children).borrow_mut()
    }

    /// Returns an iterator that yields ancestor nodes (including itself).
    pub fn ancestors(self) -> impl Iterator<Item = XmlNode<'gc>> {
        core::iter::successors(Some(self), |node| node.parent())
    }

    /// Get the already-instantiated script object from the current node.
    fn get_script_object(self) -> Option<Object<'gc>> {
        self.0.script_object.get()
    }

    /// Introduce this node to a new script object.
    ///
    /// This internal function *will* overwrite already extant objects, so only
    /// call this if you need to instantiate the script object for the first
    /// time. Attempting to call it a second time will panic.
    pub fn introduce_script_object(self, gc_context: &Mutation<'gc>, new_object: Object<'gc>) {
        assert!(self.get_script_object().is_none(), "An attempt was made to change the already-established link between script object and XML node. This has been denied and is likely a bug.");

        unlock!(Gc::write(gc_context, self.0), XmlNodeData, script_object).set(Some(new_object));
    }

    /// Obtain the script object for a given XML tree node, constructing a new
    /// script object if one does not exist.
    pub fn script_object(self, activation: &mut Activation<'_, 'gc>) -> Object<'gc> {
        match self.get_script_object() {
            Some(object) => object,
            None => {
                let xml_node = activation.context.avm1.prototypes().xml_node_constructor;
                let prototype = xml_node
                    .get(istr!("prototype"), activation)
                    .map(|p| p.coerce_to_object(activation))
                    .ok();
                let object = Object::new(&activation.context.strings, prototype);
                self.introduce_script_object(activation.gc(), object);
                object.set_native(activation.gc(), NativeObject::XmlNode(self));
                object
            }
        }
    }

    /// Obtain the script object for a given XML tree node's attributes.
    pub fn attributes(&self) -> Object<'gc> {
        self.0.attributes
    }

    /// Gets a lazy-created .childNodes array
    pub fn get_or_init_cached_child_nodes(
        &self,
        activation: &mut Activation<'_, 'gc>,
    ) -> Result<Object<'gc>, Error<'gc>> {
        let array = self.0.cached_child_nodes.get();
        if let Some(array) = array {
            Ok(array)
        } else {
            let array = ArrayBuilder::empty(activation);
            unlock!(
                Gc::write(activation.gc(), self.0),
                XmlNodeData,
                cached_child_nodes
            )
            .set(Some(array));
            self.refresh_cached_child_nodes(activation)?;
            Ok(array)
        }
    }

    /// Refreshes the .childNodes array. Call this after every child list mutation.
    pub fn refresh_cached_child_nodes(
        &self,
        activation: &mut Activation<'_, 'gc>,
    ) -> Result<(), Error<'gc>> {
        let array = self.0.cached_child_nodes.get();
        if let Some(array) = array {
            array.set_length(activation, 0)?;
            for (i, child) in self.children().enumerate() {
                let value = child.script_object(activation).into();
                array.set_element(activation, i as i32, value)?;
            }
        }
        Ok(())
    }

    /// Create a duplicate copy of this node.
    ///
    /// If the `deep` flag is set true, then the entire node tree will be cloned.
    pub fn duplicate(self, gc_context: &Mutation<'gc>, deep: bool) -> Self {
        let attributes = Object::new_without_proto(gc_context);
        for (key, value) in self.attributes().own_properties() {
            attributes.define_value(gc_context, key, value, Attribute::empty());
        }

        let clone = Self(Gc::new(
            gc_context,
            XmlNodeData {
                script_object: Lock::new(None),
                parent: Lock::new(None),
                prev_sibling: Lock::new(None),
                next_sibling: Lock::new(None),
                node_type: self.node_type(),
                node_value: Lock::new(self.0.node_value.get()),
                attributes,
                cached_child_nodes: Lock::new(None),
                children: RefLock::new(Vec::new()),
            },
        ));

        if deep {
            for (position, child) in self.children().enumerate() {
                clone.insert_child(gc_context, position, child.duplicate(gc_context, deep));
            }
        }

        clone
    }

    /// Look up the URI for the given prefix.
    ///
    /// XML namespaces are determined by `xmlns:` namespace attributes on the
    /// current node, or its parent.
    pub fn lookup_namespace_uri(self, prefix: &WStr) -> Option<Value<'gc>> {
        for node in self.ancestors() {
            // Iterate attributes by their definition order, so the first matching one
            // is returned.
            for (key, value) in node.attributes().own_properties() {
                if let Some(namespace) = key.strip_prefix(WStr::from_units(b"xmlns")) {
                    // The empty prefix matches every attribute that starts with "xmlns"
                    // (with/without a colon).
                    if prefix.is_empty() || namespace.strip_prefix(b':') == Some(prefix) {
                        return Some(value);
                    }
                }
            }
        }

        None
    }

    /// Convert the given node to a string of UTF-8 encoded XML.
    pub fn into_string(self, activation: &mut Activation<'_, 'gc>) -> Result<WString, Error<'gc>> {
        let mut result = WString::new();
        self.write_node_to_string(activation, &mut result)?;
        Ok(result)
    }

    /// Write the contents of this node, including its children, to the given string.
    fn write_node_to_string(
        self,
        activation: &mut Activation<'_, 'gc>,
        result: &mut WString,
    ) -> Result<(), Error<'gc>> {
        // TODO: we convert some strings to utf8, replacing unpaired surrogates by the replacement char.
        // It is correct?
        if self.0.node_type == ELEMENT_NODE {
            let children = self.0.children.borrow();
            if let Some(tag_name) = self.0.node_value.get() {
                result.push_byte(b'<');
                result.push_str(&tag_name);

                for (key, value) in self.attributes().own_properties() {
                    let value = value.coerce_to_string(activation)?;
                    let value = value.to_utf8_lossy();
                    let value = escape(&*value);

                    result.push_byte(b' ');
                    result.push_str(&key);
                    result.push_str(WStr::from_units(b"=\""));
                    result.push_utf8(&value);
                    result.push_byte(b'"');
                }

                if children.is_empty() {
                    result.push_str(WStr::from_units(b" />"));
                } else {
                    result.push_byte(b'>');
                    for child in &*children {
                        child.write_node_to_string(activation, result)?;
                    }
                    result.push_str(WStr::from_units(b"</"));
                    result.push_str(&tag_name);
                    result.push_byte(b'>');
                }
            } else {
                for child in &*children {
                    child.write_node_to_string(activation, result)?;
                }
            }
        } else {
            let value = self.0.node_value.get().unwrap();
            let value = value.to_utf8_lossy();
            let value = escape(&*value);
            result.push_utf8(&value);
        }

        Ok(())
    }
}

impl fmt::Debug for XmlNode<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("XmlNodeData")
            .field("ptr", &Gc::as_ptr(self.0))
            .field(
                "script_object",
                &self
                    .0
                    .script_object
                    .get()
                    .map(|p| format!("{:p}", p.as_ptr()))
                    .unwrap_or_else(|| "None".to_string()),
            )
            .field(
                "parent",
                &self
                    .parent()
                    .map(|p| format!("{:p}", Gc::as_ptr(p.0)))
                    .unwrap_or_else(|| "None".to_string()),
            )
            .field("node_value", &self.0.node_value.get())
            .field("attributes", &self.0.attributes)
            .field("children", &self.0.children.borrow())
            .finish()
    }
}
