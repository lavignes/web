use std::io::Cursor;

use super::*;

#[test]
fn test() {
    let reader = Cursor::new("<section><b>foo</p>hello world!");
    let dom = Dom::new();
    let mut html = Html::new(reader, dom);
    html.parse().unwrap();

    let dom = html.into_dom();
    dom.write_tree(&mut io::stdout()).unwrap();
    dom.write_junk(&mut io::stdout()).unwrap();
}
