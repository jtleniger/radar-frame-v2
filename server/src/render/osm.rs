//! Parse Overpass OSM XML into highway polylines.
//!
//! The Overpass query in `server/sources/osm.py` returns ways tagged
//! `highway=motorway|primary` (including `_link` variants) and the nodes they
//! reference. This module extracts those ways as polylines in the WGS84
//! lon/lat order expected by the rasterizer.

use std::collections::HashMap;
use std::io::Cursor;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use crate::render::{Error, Result};

/// A parsed OSM node: id -> [lon, lat].
type Nodes = HashMap<i64, [f64; 2]>;

/// Reject a body whose document element is not `<osm>`.
///
/// Overpass can answer with an HTML error page and status `200`; that page
/// parses as well-formed XML with no highway ways, so the root check is what
/// keeps it from being mistaken for an empty street mask.
pub fn validate_root(xml: &str) -> Result<()> {
    let mut reader = Reader::from_reader(Cursor::new(xml));
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if e.name().as_ref() == b"osm" {
                    return Ok(());
                }
                return Err(Error(format!(
                    "OSM XML root is <{}>, not <osm>",
                    String::from_utf8_lossy(e.name().as_ref())
                )));
            }
            Ok(Event::Eof) => return Err(Error("OSM XML has no <osm> root".into())),
            Ok(_) => {}
            Err(e) => {
                return Err(Error(format!(
                    "OSM XML parse error at position {}: {e}",
                    reader.buffer_position()
                )))
            }
        }
        buf.clear();
    }
}

/// Parse Overpass OSM XML and return the highway polylines.
///
/// Only ways whose `highway` tag contains the substring `motorway` or `primary`
/// are included, matching the Overpass filter used by `sources/osm.py`.
/// Missing node references produce an error that names the way id.
pub fn parse_polylines(xml: &str) -> Result<Vec<Vec<[f64; 2]>>> {
    let mut reader = Reader::from_reader(Cursor::new(xml));
    reader.config_mut().trim_text(true);

    let mut nodes: Nodes = HashMap::new();
    let mut ways: Vec<Way> = Vec::new();
    let mut current_way: Option<Way> = None;
    let mut buf = Vec::new();

    loop {
        match &reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) if e.name().as_ref() == b"node" => {
                parse_node(e, &mut nodes)?;
            }
            Ok(Event::Start(e)) if e.name().as_ref() == b"way" => {
                current_way = Some(parse_way_start(e)?);
            }
            Ok(Event::Empty(e)) if e.name().as_ref() == b"nd" => {
                if let Some(way) = &mut current_way {
                    parse_nd(e, way)?;
                }
            }
            Ok(Event::Empty(e)) if e.name().as_ref() == b"tag" => {
                if let Some(way) = &mut current_way {
                    parse_tag(e, way);
                }
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"way" => {
                if let Some(way) = current_way.take() {
                    ways.push(way);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => {
                return Err(Error(format!(
                    "OSM XML parse error at position {}: {e}",
                    reader.buffer_position()
                )));
            }
        }
        buf.clear();
    }

    let mut polylines = Vec::with_capacity(ways.len());
    for way in ways {
        if !way.is_highway_match() {
            continue;
        }
        if way.nodes.is_empty() {
            continue;
        }
        let mut points = Vec::with_capacity(way.nodes.len());
        for node_id in &way.nodes {
            match nodes.get(node_id) {
                Some(point) => points.push(*point),
                None => {
                    return Err(Error(format!(
                        "OSM way {} references missing node {}",
                        way.id, node_id
                    )));
                }
            }
        }
        polylines.push(points);
    }

    Ok(polylines)
}

struct Way {
    id: i64,
    nodes: Vec<i64>,
    highway: Option<String>,
}

impl Way {
    fn is_highway_match(&self) -> bool {
        match &self.highway {
            Some(value) => value.contains("motorway") || value.contains("primary"),
            None => false,
        }
    }
}

fn parse_node(e: &BytesStart, nodes: &mut Nodes) -> Result<()> {
    let id = required_attr_i64(e, b"id")?;
    let lon = required_attr_f64(e, b"lon")?;
    let lat = required_attr_f64(e, b"lat")?;
    nodes.insert(id, [lon, lat]);
    Ok(())
}

fn parse_way_start(e: &BytesStart) -> Result<Way> {
    let id = required_attr_i64(e, b"id")?;
    Ok(Way {
        id,
        nodes: Vec::new(),
        highway: None,
    })
}

fn parse_nd(e: &BytesStart, way: &mut Way) -> Result<()> {
    let node_id = required_attr_i64(e, b"ref")?;
    way.nodes.push(node_id);
    Ok(())
}

fn parse_tag(e: &BytesStart, way: &mut Way) {
    if let (Ok(Some(k)), Ok(Some(v))) = (attr_str(e, b"k"), attr_str(e, b"v")) {
        if k == "highway" {
            way.highway = Some(v);
        }
    }
}

fn required_attr_i64(e: &BytesStart, name: &[u8]) -> Result<i64> {
    attr_str(e, name)?
        .ok_or_else(|| {
            Error(format!(
                "missing '{}' attribute",
                std::str::from_utf8(name).unwrap_or("?")
            ))
        })?
        .parse()
        .map_err(|e| {
            Error(format!(
                "invalid '{}' attribute: {e}",
                std::str::from_utf8(name).unwrap_or("?")
            ))
        })
}

fn required_attr_f64(e: &BytesStart, name: &[u8]) -> Result<f64> {
    attr_str(e, name)?
        .ok_or_else(|| {
            Error(format!(
                "missing '{}' attribute",
                std::str::from_utf8(name).unwrap_or("?")
            ))
        })?
        .parse()
        .map_err(|e| {
            Error(format!(
                "invalid '{}' attribute: {e}",
                std::str::from_utf8(name).unwrap_or("?")
            ))
        })
}

fn attr_str(e: &BytesStart, name: &[u8]) -> Result<Option<String>> {
    for attr in e.attributes() {
        let attr = attr.map_err(|e| Error(format!("attribute error: {e}")))?;
        if attr.key.as_ref() == name {
            let value = attr
                .unescape_value()
                .map_err(|e| Error(format!("attribute value error: {e}")))?;
            return Ok(Some(value.into_owned()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_highway_ways_and_skips_others() {
        let xml = r#"<?xml version="1.0"?>
<osm>
  <node id="1" lat="35.0" lon="-97.0"/>
  <node id="2" lat="35.1" lon="-97.1"/>
  <node id="3" lat="35.2" lon="-97.2"/>
  <way id="10">
    <nd ref="1"/>
    <nd ref="2"/>
    <tag k="highway" v="primary"/>
  </way>
  <way id="11">
    <nd ref="2"/>
    <nd ref="3"/>
    <tag k="highway" v="residential"/>
  </way>
  <way id="12">
    <nd ref="1"/>
    <nd ref="3"/>
    <tag k="highway" v="motorway_link"/>
  </way>
</osm>"#;

        let polylines = parse_polylines(xml).unwrap();
        assert_eq!(polylines.len(), 2);
        assert_eq!(polylines[0], vec![[-97.0, 35.0], [-97.1, 35.1]]);
        assert_eq!(polylines[1], vec![[-97.0, 35.0], [-97.2, 35.2]]);
    }

    #[test]
    fn rejects_a_document_that_is_not_osm() {
        let error = validate_root("<html><body>Overpass error</body></html>")
            .unwrap_err()
            .to_string();
        assert!(error.contains("<html>"), "{error}");
        assert!(error.contains("not <osm>"), "{error}");
    }

    #[test]
    fn accepts_an_empty_osm_document() {
        validate_root("<?xml version=\"1.0\"?><osm version=\"0.6\"></osm>").unwrap();
    }

    #[test]
    fn errors_on_missing_node_ref() {
        let xml = r#"<?xml version="1.0"?>
<osm>
  <node id="1" lat="35.0" lon="-97.0"/>
  <way id="10">
    <nd ref="1"/>
    <nd ref="99"/>
    <tag k="highway" v="primary"/>
  </way>
</osm>"#;

        let err = parse_polylines(xml).unwrap_err().to_string();
        assert!(err.contains("way 10"), "{err}");
        assert!(err.contains("node 99"), "{err}");
    }

    #[test]
    fn ignores_ways_without_highway_tag() {
        let xml = r#"<?xml version="1.0"?>
<osm>
  <node id="1" lat="35.0" lon="-97.0"/>
  <node id="2" lat="35.1" lon="-97.1"/>
  <way id="10">
    <nd ref="1"/>
    <nd ref="2"/>
  </way>
</osm>"#;

        assert!(parse_polylines(xml).unwrap().is_empty());
    }
}
