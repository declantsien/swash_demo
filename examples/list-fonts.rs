extern crate font_index;

use font_index::FontCache;
use font_index::FontIndex;
use font_index::GenericFamily;
use swash::{Attributes, Stretch, Style, Weight};

fn main() {

    // let family_names: Vec<String> = FontIndex::global()
    //     .families
    //     .clone()
    //     .iter()
    //     .map(|data| data.name.to_string())
    //     .collect();
    // println!("names {:?}", family_names);

    let stretch = Stretch::NORMAL;
    let style = Style::Normal;
    let weight = Weight::NORMAL;

    if let Some(font) = FontIndex::global().query(
        "Droid Sans Mono",
        Attributes::new(stretch, weight, style),
    ) {
        println!("font {:?} {:?}", font.family_name(), font.attributes());
    }

    if let Some(family) = FontIndex::global().family_by_key("serif") {
        family
            .fonts()
            .for_each(|font| println!("font {:?} {:?}", font.family_name(), font.attributes()))
    }


    let mut cache = FontCache::default();
    if let Some(font) = cache.query("serif", Attributes::new(stretch, weight, style)) {
        println!(
            "data {:?}",
            &cache.get(font.id()).unwrap().as_ref().data[..50]
        );
    }
}
