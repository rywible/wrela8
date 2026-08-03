use std::path::Path;
use std::process::Command;

#[test]
fn renderer_selector_rejects_an_image_without_renderers() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden/check-pixels-empty/input.wr");
    let expected = "error[pixels]: renderer index 0 is out of range; image declares 0 renderers\n";

    for stage in ["field-graph", "frame-program", "render-layout"] {
        let output = Command::new(env!("CARGO_BIN_EXE_wrela"))
            .args([
                "dump",
                &format!("--stage={stage}"),
                "--renderer=0",
                source.to_str().expect("fixture path is UTF-8"),
            ])
            .output()
            .expect("run the wrela CLI");

        assert!(
            !output.status.success(),
            "{stage} unexpectedly accepted a renderer selector for an empty image"
        );
        assert_eq!(
            String::from_utf8(output.stdout).expect("CLI stdout is UTF-8"),
            ""
        );
        assert_eq!(
            String::from_utf8(output.stderr).expect("CLI stderr is UTF-8"),
            expected
        );
    }
}
