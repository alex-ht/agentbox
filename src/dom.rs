//! Minimal DOM helpers on top of html5ever (already used by htmd), for
//! scraping search result pages without pulling in a selector engine.

use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};

pub fn parse(html: &str) -> Handle {
    let dom = html5ever::parse_document(RcDom::default(), Default::default()).one(html);
    dom.document
}

pub fn tag(h: &Handle) -> Option<String> {
    match &h.data {
        NodeData::Element { name, .. } => Some(name.local.to_string()),
        _ => None,
    }
}

pub fn attr(h: &Handle, key: &str) -> Option<String> {
    match &h.data {
        NodeData::Element { attrs, .. } => attrs
            .borrow()
            .iter()
            .find(|a| &a.name.local == key)
            .map(|a| a.value.to_string()),
        _ => None,
    }
}

pub fn has_class(h: &Handle, class: &str) -> bool {
    attr(h, "class").is_some_and(|c| c.split_whitespace().any(|x| x == class))
}

pub fn class_starts_with(h: &Handle, prefix: &str) -> bool {
    attr(h, "class").is_some_and(|c| c.split_whitespace().any(|x| x.starts_with(prefix)))
}

/// All descendants (pre-order) matching `pred`.
pub fn find_all(root: &Handle, pred: &dyn Fn(&Handle) -> bool) -> Vec<Handle> {
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(node) = stack.pop() {
        if pred(&node) {
            out.push(node.clone());
        }
        let children = node.children.borrow();
        for c in children.iter().rev() {
            stack.push(c.clone());
        }
    }
    out
}

pub fn find_first(root: &Handle, pred: &dyn Fn(&Handle) -> bool) -> Option<Handle> {
    find_all(root, pred).into_iter().next()
}

/// Visible text of a node with whitespace collapsed (script/style skipped).
pub fn text(h: &Handle) -> String {
    let mut buf = String::new();
    collect_text(h, &mut buf);
    buf.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn collect_text(h: &Handle, buf: &mut String) {
    match &h.data {
        NodeData::Text { contents } => {
            buf.push_str(&contents.borrow());
        }
        NodeData::Element { name, .. }
            if matches!(&*name.local, "script" | "style" | "noscript") => {}
        _ => {
            for c in h.children.borrow().iter() {
                collect_text(c, buf);
            }
            if matches!(tag(h).as_deref(), Some("p" | "div" | "br" | "li")) {
                buf.push(' ');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_by_class_and_reads_text() {
        let doc = parse(
            r#"<div class="a b"><p>Hello <b>world</b></p><script>x()</script><a href="/u" class="link">L</a></div>"#,
        );
        let divs = find_all(&doc, &|n| has_class(n, "b"));
        assert_eq!(divs.len(), 1);
        assert_eq!(text(&divs[0]), "Hello world L");
        let a = find_first(&doc, &|n| tag(n).as_deref() == Some("a")).unwrap();
        assert_eq!(attr(&a, "href").as_deref(), Some("/u"));
        assert!(class_starts_with(&divs[0], "a"));
    }
}
