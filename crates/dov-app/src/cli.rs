//! `destructovision module …`: tools for module authors.

use std::path::{Path, PathBuf};

pub const USAGE: &str = "\
usage: destructovision module <command>
  check PATH...                  check module files, panel art, packages, or folders of them
  new NAMESPACE.NAME [--linescan] [--dir DIR]
                                 start a module file (in the modules folder by default)
  glue FILE                      print the complete shader the engine builds from a module";

/// Runs a `module` subcommand; returns the process exit code.
pub fn module(args: &[String], modules_dir: &Path) -> i32 {
    let result = match args.first().map(String::as_str) {
        Some("check") if args.len() > 1 => Ok(check(&args[1..])),
        Some("new") if args.len() > 1 => new(&args[1..], modules_dir),
        Some("glue") if args.len() == 2 => glue(Path::new(&args[1])),
        _ => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn check(paths: &[String]) -> i32 {
    let mut failed = false;
    for path in paths.iter().map(PathBuf::from) {
        let is_svg = path.extension().is_some_and(|e| e == "svg");
        let scan = if path.join(dov_format::PACKAGE_FILE).is_file() {
            dov_modules::load_package(&path)
        } else if path.is_dir() {
            dov_modules::load_dir(&path)
        } else if is_svg {
            dov_modules::Scan::default()
        } else {
            dov_modules::load_files(std::slice::from_ref(&path))
        };
        for ty in &scan.loaded {
            let file = match &ty.origin {
                dov_modules::Origin::File { path, .. } => path.display().to_string(),
                _ => String::new(),
            };
            println!("ok     {}  {file}", ty.desc.id);
            // Installed in this process only, so full art for these modules
            // is checked against their controls.
            dov_modules::install(ty);
        }
        for (path, e) in &scan.errors {
            failed = true;
            println!("error  {}\n{}\n", path.display(), indent(e));
        }
        let art = if is_svg {
            vec![path.clone()]
        } else {
            crate::art::files_under(&path)
        };
        for file in &art {
            match crate::art::check_file(file) {
                Ok(what) => println!("ok     panel {what}  {}", file.display()),
                Err(e) => {
                    failed = true;
                    println!("error  {}\n{}\n", file.display(), indent(&e));
                }
            }
        }
        if scan.loaded.is_empty() && scan.errors.is_empty() && art.is_empty() {
            failed = true;
            println!("error  {}: no module files or panel art found", path.display());
        }
    }
    i32::from(failed)
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|l| format!("       {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn new(args: &[String], modules_dir: &Path) -> Result<i32, String> {
    let id = &args[0];
    if !dov_format::module::is_module_id(id) || id.starts_with("dov.") {
        return Err(format!(
            "`{id}` must be `<namespace>.<name>`, lowercase with dashes, like `alice.vhs-smear` \
             (the `dov` namespace is reserved)"
        ));
    }
    let mut linescan = false;
    let mut dir = modules_dir.to_path_buf();
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--linescan" => linescan = true,
            "--dir" => dir = PathBuf::from(rest.next().ok_or("--dir needs a folder")?),
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    let name = id.split_once('.').map_or(id.as_str(), |(_, n)| n);
    let path = dir.join(format!("{name}.wgsl"));
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::write(&path, crate::modules::template(id, linescan)).map_err(|e| e.to_string())?;
    println!("{}", path.display());
    Ok(0)
}

fn glue(path: &Path) -> Result<i32, String> {
    let source = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let module = dov_format::parse(&source).map_err(|e| format!("{}:\n{e}", path.display()))?;
    print!("{}", dov_format::glue::shader(&module.interface, &module.source));
    Ok(0)
}
