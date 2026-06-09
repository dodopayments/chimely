//! Code-first OpenAPI document (utoipa).
//!
//! Contract rule (see CLAUDE.md): until v1, `specs/openapi.yaml` is the
//! convergence target. CI exports this document (`dronte openapi`) and runs
//! oasdiff against the spec; the diff is the to-do list. Handlers added in
//! Phase 1/2 register their `#[utoipa::path]` here until the diff is empty.

use utoipa::OpenApi;

/// Title/version deliberately mirror specs/openapi.yaml so the oasdiff delta
/// is only the parts we haven't built yet, not metadata noise.
#[derive(OpenApi)]
#[openapi(
    info(title = "Dronte API", version = "1.0.0"),
    tags(
        (name = "management", description = "Backend-to-Dronte. Bearer API key."),
        (name = "subscriber", description = "Widget-to-Dronte. HMAC subscriber hash.")
    )
)]
pub struct ApiDoc;

pub fn api_doc() -> utoipa::openapi::OpenApi {
    let mut doc = ApiDoc::openapi();
    // utoipa fills these from Cargo.toml metadata (empty here); drop them
    // rather than exporting empty strings. The spec's long-form description
    // is added in Phase 1 together with the endpoints.
    doc.info.description = None;
    doc.info.license = None;
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_yaml_with_expected_identity() {
        let yaml = api_doc().to_yaml().expect("spec serializes");
        assert!(yaml.contains("title: Dronte API"));
        assert!(yaml.contains("version: 1.0.0"));
    }

    #[test]
    fn declares_both_planes_as_tags() {
        let doc = api_doc();
        let tags: Vec<String> = doc
            .tags
            .unwrap_or_default()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(tags, ["management", "subscriber"]);
    }
}
