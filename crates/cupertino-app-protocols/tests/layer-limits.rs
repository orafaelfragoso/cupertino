//! The limits of `cupertino_layers_v1` are the ones its XML documents.

use cupertino_app_protocols::layers::*;

const XML: &str = include_str!("../protocols/cupertino-layers-v1.xml");

/// The XML with its lines joined, so a phrase broken across lines is found.
fn text() -> String {
    XML.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn the_xml_documents_every_limit() {
    let text = text();
    let phrases = [
        format!(
            "a duration over {} seconds",
            f64::from(MAX_DURATION_MS) / 1000.0
        ),
        format!("milliseconds, at most {MAX_DURATION_MS}"),
        format!("milliseconds, 1 to {MAX_DURATION_MS}"),
        format!(
            "a spring response outside {} to {} seconds",
            SPRING_RESPONSE.0, SPRING_RESPONSE.1
        ),
        format!(
            "damping ratio outside {} to {}",
            SPRING_DAMPING.0, SPRING_DAMPING.1
        ),
        format!("a lens outside {} to {}", LENS.0, LENS.1),
        format!("a lens below {} or above {}", LENS.0, LENS.1),
        format!("a glass group spacing outside 0 to {MAX_GROUP_SPACING}"),
        format!("a negative spacing or one above {MAX_GROUP_SPACING}"),
        format!("a content blur over {MAX_CONTENT_BLUR}"),
        format!("a glass edge wider than {MAX_GLASS_EDGE}"),
        format!("below 0 or above {MAX_GLASS_EDGE}"),
        format!("{MIN_SAMPLES} to {MAX_SAMPLES} of"),
        format!("fewer than {MIN_SAMPLES} or more than {MAX_SAMPLES} samples"),
    ];
    for phrase in phrases {
        assert!(text.contains(&phrase), "the XML does not say {phrase:?}");
    }
}
