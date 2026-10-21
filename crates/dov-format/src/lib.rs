//! The Destruct-O-Vision module format.
//!
//! A module is one WGSL file whose first comment is a TOML header naming its
//! ports and params. The engine wraps the module's `process` function in
//! generated glue: bindings, the `Ctx`/`Inputs`/`Params`/`Outputs` structs,
//! CV handling, and the compute entry point. Modules carry no native code.
//!
//! ```wgsl
//! /* dov-module
//! format = 1
//! id = "alice.posterize"
//! name = "Posterize"
//! category = "process"
//! description = "Quantizes each channel to a few levels."
//! kernel = "pointwise"
//!
//! [[input]]
//! id = "sig"
//! kind = "rgb"
//!
//! [[output]]
//! id = "out"
//! kind = "rgb"
//!
//! [[param]]
//! id = "levels"
//! range = [2.0, 32.0]
//! default = 4.0
//! curve = "exp"
//! cv = "video"
//! */
//!
//! fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
//!     let levels = max(round(p.levels), 2.0);
//!     var o: Outputs;
//!     o.out = floor(clamp(inp.sig, vec3<f32>(0.0), vec3<f32>(0.9999)) * levels) / (levels - 1.0);
//!     return o;
//! }
//! ```
//!
//! - [`module::parse`] reads a file's header.
//! - [`module::check`] also compiles the module against the glue.
//! - [`package::parse_package`] reads a package's `package.toml`.
//! - [`glue::shader`] builds the shader the engine runs.
//!
//! This crate is MIT-licensed, unlike the app, so authoring tools, the module
//! registry and other hosts can use it freely.

pub mod fs;
pub mod glue;
pub mod interface;
pub mod module;
pub mod package;
pub mod panel;

pub use fs::read_text_file;
pub use interface::{Curve, Cv, History, Interface, KernelKind, Param, ParamKind, Port, PortKind};
pub use module::{Category, Error, FORMAT, MAX_TEXT_CHARS, Module, check, parse};
pub use package::{PACKAGE_FILE, Package, parse_package};
pub use usvg;

#[cfg(test)]
mod tests {
    use super::*;

    const POSTERIZE: &str = r#"/* dov-module
format = 1
id = "alice.posterize"
name = "Posterize"
category = "process"
tags = ["color"]
description = "Quantizes each channel to a few levels."
kernel = "pointwise"

[[input]]
id = "sig"
name = "In"
kind = "rgb"

[[output]]
id = "out"
kind = "rgb"

[[param]]
id = "levels"
range = [2.0, 32.0]
default = 4.0
curve = "exp"
cv = "video"

[[param]]
id = "mode"
choices = ["All", "Luma"]
default = "Luma"
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let levels = max(round(p.levels), 2.0);
    var o: Outputs;
    o.out = floor(clamp(inp.sig, vec3<f32>(0.0), vec3<f32>(0.9999)) * levels) / (levels - 1.0);
    if (p.mode == 1u && patched_sig()) {
        o.out = vec3<f32>(luma(o.out));
    }
    return o;
}
"#;

    fn errors(source: &str) -> String {
        check(source, "test.wgsl").unwrap_err().to_string()
    }

    #[test]
    fn a_valid_module_checks() {
        let m = check(POSTERIZE, "posterize.wgsl").unwrap();
        assert_eq!(m.id, "alice.posterize");
        assert_eq!(m.namespace(), "alice");
        assert_eq!(m.category, Category::Process);
        let i = &m.interface;
        assert_eq!(i.inputs[0].name, "In");
        assert_eq!(i.outputs[0].name, "out", "name defaults to the id");
        assert_eq!(i.inputs_full()[1].id, "levels_cv");
        assert_eq!(i.cv_slot(0), Some((1, 2)));
        assert!(matches!(i.params[1].kind, ParamKind::Choice { default: 1, .. }));
    }

    #[test]
    fn header_errors_point_at_their_line() {
        let src = POSTERIZE.replace("kernel = \"pointwise\"", "kernel = \"pointwise\"\ncolour = 3");
        let e = errors(&src);
        assert!(e.contains("line 9") && e.contains("colour"), "{e}");
    }

    #[test]
    fn shader_errors_point_at_the_authors_line() {
        let src = POSTERIZE.replace("o.out = vec3<f32>(luma(o.out));", "o.out = p.nope;");
        let e = errors(&src);
        let line = src.lines().position(|l| l.contains("p.nope")).unwrap() + 1;
        assert!(e.contains(&format!("test.wgsl:{line}:")), "{e}");
    }

    #[test]
    fn interface_mistakes_are_all_reported() {
        let src = POSTERIZE
            .replace("id = \"alice.posterize\"", "id = \"Alice/posterize\"")
            .replace("range = [2.0, 32.0]", "range = [0.0, 32.0]")
            .replace("default = \"Luma\"", "default = \"Chroma\"")
            .replace("id = \"sig\"", "id = \"filter\"");
        let e = errors(&src);
        for expected in [
            "Alice/posterize",
            "exp curve needs min > 0",
            "`Chroma`",
            "input id `filter`",
        ] {
            assert!(e.contains(expected), "missing {expected:?} in:\n{e}");
        }
    }

    #[test]
    fn generated_cv_jacks_cant_collide() {
        let src = POSTERIZE
            .replace("id = \"sig\"", "id = \"levels_cv\"")
            .replace("inp.sig", "inp.levels_cv")
            .replace("patched_sig", "patched_levels_cv");
        assert!(errors(&src).contains("`levels_cv` is declared twice"));
    }

    #[test]
    fn modules_cant_declare_resources_variables_or_reserved_names() {
        let src = format!(
            "{POSTERIZE}\n@group(0) @binding(9) var<storage, read_write> leak: array<u32>;\nvar<private> state: f32;\nfn luma(c: vec3<f32>) -> f32 {{ return 0.0; }}\nfn fetch_x() {{}}\n"
        );
        let e = errors(&src);
        for expected in [
            "can't declare resources",
            "module-scope variables",
            "`luma` is a reserved name",
            "`fetch_x` is a reserved name",
        ] {
            assert!(e.contains(expected), "missing {expected:?} in:\n{e}");
        }
    }

    #[test]
    fn the_header_must_come_first_and_close() {
        assert!(errors(&format!("// hi\n{POSTERIZE}")).contains("must start with"));
        assert!(errors(&POSTERIZE.replacen("*/", "", 1)).contains("never closed"));
        let sneaky = POSTERIZE.replace("Quantizes", "Quantizes */ fn x() {} /*");
        assert!(errors(&sneaky).contains("can't contain"));
    }

    #[test]
    fn linescan_needs_state() {
        let src = POSTERIZE.replace("kernel = \"pointwise\"", "kernel = \"linescan\"");
        assert!(errors(&src).contains("struct State"));
    }

    #[test]
    fn packages_name_their_modules() {
        let dir = std::env::temp_dir().join(format!("dov-format-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("modules")).unwrap();
        std::fs::write(dir.join("modules/a.wgsl"), POSTERIZE).unwrap();
        std::fs::write(dir.join("modules/notes.txt"), "").unwrap();
        let p = parse_package(
            "[package]\nid = \"alice.pack\"\nname = \"Pack\"\nversion = \"1.2.0\"\nlicense = \"MIT\"\n",
        )
        .unwrap();
        assert_eq!(p.namespace(), "alice");
        assert_eq!(p.module_files(&dir), [dir.join("modules/a.wgsl")]);
        let bad = parse_package(
            "[package]\nid = \"alice.pack\"\nname = \"P\"\nversion = \"1.2\"\nmodules = [\"../x/*\"]\n",
        );
        let e = bad.unwrap_err().to_string();
        assert!(e.contains("semver") && e.contains("inside the package"), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
