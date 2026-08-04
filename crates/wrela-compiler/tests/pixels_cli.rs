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

#[test]
fn renderer_selector_is_required_for_ambiguous_images_and_dumps_only_the_selection() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/golden/check-pixels-two-renderers/src/examples/check_pixels_two_renderers.wr",
    );

    let ambiguous = Command::new(env!("CARGO_BIN_EXE_wrela"))
        .args([
            "dump",
            "--stage=field-graph",
            source.to_str().expect("fixture path is UTF-8"),
        ])
        .output()
        .expect("run the wrela CLI");
    assert!(!ambiguous.status.success());
    assert!(
        String::from_utf8(ambiguous.stderr)
            .unwrap()
            .contains("use --renderer=<index>")
    );

    let selected = Command::new(env!("CARGO_BIN_EXE_wrela"))
        .args([
            "dump",
            "--stage=field-graph",
            "--renderer=1",
            source.to_str().expect("fixture path is UTF-8"),
        ])
        .output()
        .expect("run the wrela CLI");
    assert!(selected.status.success());
    let stdout = String::from_utf8(selected.stdout).unwrap();
    assert!(stdout.contains("Renderers count=2"));
    assert!(stdout.contains("Renderer index=1"));
    assert!(!stdout.contains("Renderer index=0"));
}

#[test]
fn unsupported_renderer_fails_before_emitting_a_partial_field_graph() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/golden/err-pixels-unsupported-op/src/examples/err_pixels_unsupported_op.wr",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_wrela"))
        .args([
            "dump",
            "--stage=field-graph",
            source.to_str().expect("fixture path is UTF-8"),
        ])
        .output()
        .expect("run the wrela CLI");

    assert!(!output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("error[pixels P004]:"));
    assert!(!stderr.contains("FieldGraph v1"));
}

#[test]
fn build_runs_symbolic_compilation_and_rejects_symbolic_only_errors() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden/check-pixels-plane/src/examples/check_pixels_plane.wr");
    let source = std::fs::read_to_string(fixture).unwrap();
    let helpers = (0..130)
        .map(|index| {
            if index == 129 {
                format!("fn symbolic_depth_{index}(value: u8) -> u8:\n    return value\n")
            } else {
                format!(
                    "fn symbolic_depth_{index}(value: u8) -> u8:\n    return symbolic_depth_{}(value)\n",
                    index + 1
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let source = source.replace("@field\nfn world", &format!("{helpers}\n@field\nfn world"));
    let source = source.replace(
        "fn world(p: Vec3, read params: SceneParams) -> Field:\n",
        "fn world(p: Vec3, read params: SceneParams) -> Field:\n    ignored: u8 = symbolic_depth_0(0)\n",
    );
    let temp = std::env::temp_dir().join(format!("wrela-p2-build-gate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(temp.join("examples")).unwrap();
    let input = temp.join("examples/check_pixels_plane.wr");
    std::fs::write(&input, source).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_wrela"))
        .args([
            "build",
            input.to_str().expect("temporary path is UTF-8"),
            "--out-dir",
            temp.to_str().expect("temporary path is UTF-8"),
        ])
        .output()
        .expect("run the wrela CLI");
    let _ = std::fs::remove_dir_all(&temp);

    assert!(!output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("error[pixels P014]:"), "{stderr}");
    assert!(stderr.contains("call-depth quota"), "{stderr}");
}
