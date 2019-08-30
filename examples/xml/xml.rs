#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

extern crate azul;

use azul::prelude::*;

macro_rules! XML_PATH { () => (concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/xml/ui.xml")) }
macro_rules! CSS_PATH { () => (concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/xml/xml.css")) }

struct DataModel { }

impl Layout for DataModel {
    fn layout(&self, _: LayoutInfo) -> Dom<DataModel> {
        DomXml::from_file(XML_PATH!(), &mut XmlComponentMap::default()).into()
    }
}
 fn main() {

    let app = App::new(DataModel { }, AppConfig::default()).unwrap();

    #[cfg(debug_assertions)]
    let window = {
        use std::time::Duration;
        let hot_reloader = css::hot_reload_override_native(CSS_PATH!(), Duration::from_millis(500));
        WindowCreateOptions::new_hot_reload(hot_reloader)
    };

    #[cfg(not(debug_assertions))]
    let window = {
        let css = css::override_native(include_str!(CSS_PATH!())).unwrap();
        WindowCreateOptions::new(css)
    };

    app.run(window);
}