//! Lists this machine's sound cards and their controls, and the playback master the volume overlay would use.
//! Read-only: the devices are opened for query and no control is written.
//!
//! ```
//! cargo run -p cupertino-alsa --example list
//! ```

use cupertino_alsa::{card_indexes, select_master, Card, ElementType, OpenMode, VolumeRange, DEVICE_DIR};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = Path::new(DEVICE_DIR);
    for index in card_indexes(dir)? {
        let card = Card::open(dir, index, OpenMode::ReadOnly)?;
        let info = card.info()?;
        println!("card {index}: {} [{}] driver {}, mixer {:?}", info.name.trim(), info.id, info.driver, info.mixer_name);
        let elements = card.elements()?;
        let master = select_master(&elements).map(|(rank, info)| (rank, info.id.numid));
        for element in &elements {
            let marker = if master.is_some_and(|(_, numid)| numid == element.id.numid) { "  <- playback master" } else { "" };
            let detail = match (element.kind, element.range) {
                (ElementType::Integer, Some(range)) => {
                    let values = card.read_values(element)?;
                    let curve = card.db_curve(element)?;
                    let db = curve.as_ref().map(|c| format!(", {:.2}..{:.2} dB", f64::from(c.db(range.min, range.max, range.min)) / 100.0, f64::from(c.db(range.min, range.max, range.max)) / 100.0)).unwrap_or_default();
                    let level = VolumeRange::new(range.min, range.max, curve).map(|r| format!(", level {:.3}", r.level(values[0]))).unwrap_or_default();
                    format!("integer {}..{} values {values:?}{db}{level}", range.min, range.max)
                }
                (ElementType::Boolean, _) => format!("boolean values {:?}", card.read_values(element)?),
                (kind, _) => format!("{kind:?}"),
            };
            println!("  {:>3} {:?} {:?} #{}: {detail}{marker}", element.id.numid, element.id.interface, element.id.name, element.id.index);
        }
        if let Some((rank, numid)) = master {
            println!("  master candidate: control {numid} ({rank:?})");
        }
    }
    Ok(())
}
