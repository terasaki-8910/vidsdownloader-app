use serde::Deserialize;
use vids_core::naming::{NameTemplate, NamingContext, TargetOs, VideoInfo, compile, render};

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    template: NameTemplate,
    context: NamingContext,
    info: VideoInfo,
    outtmpl: String,
    windows_path: String,
}

#[test]
fn compiles_and_renders_every_reference_case() {
    let fixture: Fixture = serde_json::from_str(include_str!("fixtures/naming/cases.json"))
        .expect("fixture must be valid JSON");
    assert_eq!(fixture.cases.len(), 26);

    for case in fixture.cases {
        assert_eq!(
            compile(&case.template, &case.context, &case.info),
            Ok(case.outtmpl),
            "compile case {}",
            case.name
        );
        let rendered = render(&case.template, &case.context, &case.info, TargetOs::Windows)
            .unwrap_or_else(|error| panic!("render case {}: {error}", case.name));
        assert_eq!(
            rendered.join("/"),
            case.windows_path,
            "render case {}",
            case.name
        );
    }
}
