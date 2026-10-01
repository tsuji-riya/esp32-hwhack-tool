fn main() {
    linker_be_nice();
    generate_web_assets();
    println!("cargo:rustc-link-arg=-Tdefmt.x");
    // make sure linkall.x is the last linker script (otherwise might cause problems with flip-link)
    println!("cargo:rustc-link-arg=-Tlinkall.x");
}

fn linker_be_nice() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let kind = &args[1];
        let what = &args[2];

        match kind.as_str() {
            "undefined-symbol" => match what.as_str() {
                what if what.starts_with("_defmt_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `defmt` not found - make sure `defmt.x` is added as a linker script and you have included `use defmt_rtt as _;`"
                    );
                    eprintln!();
                }
                "_stack_start" => {
                    eprintln!();
                    eprintln!("💡 Is the linker script `linkall.x` missing?");
                    eprintln!();
                }
                what if what.starts_with("esp_rtos_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `esp-radio` has no scheduler enabled. Make sure you have initialized `esp-rtos` or provided an external scheduler."
                    );
                    eprintln!();
                }
                "embedded_test_linker_file_not_added_to_rustflags" => {
                    eprintln!();
                    eprintln!(
                        "💡 `embedded-test` not found - make sure `embedded-test.x` is added as a linker script for tests"
                    );
                    eprintln!();
                }
                "free"
                | "malloc"
                | "calloc"
                | "get_free_internal_heap_size"
                | "malloc_internal"
                | "realloc_internal"
                | "calloc_internal"
                | "free_internal" => {
                    eprintln!();
                    eprintln!(
                        "💡 Did you forget the `esp-alloc` dependency or didn't enable the `compat` feature on it?"
                    );
                    eprintln!();
                }
                _ => (),
            },
            // we don't have anything helpful for "missing-lib" yet
            _ => {
                std::process::exit(1);
            }
        }

        std::process::exit(0);
    }

    println!(
        "cargo:rustc-link-arg=-Wl,--error-handling-script={}",
        std::env::current_exe().unwrap().display()
    );
}

/// Embeds the built React UI (`web/dist`, gzip-compressed by `npm run build`) into the firmware.
///
/// Generates `$OUT_DIR/web_assets.rs`, included by `src/web.rs`. When the UI has not been built
/// yet a small placeholder page is embedded so that `cargo build`/`clippy` keep working.
fn generate_web_assets() {
    use std::fmt::Write as _;
    use std::path::{Path, PathBuf};

    let dist = Path::new("web/dist");
    println!("cargo:rerun-if-changed=web/dist");

    fn mime(name: &str) -> &'static str {
        match name.rsplit('.').next().unwrap_or("") {
            "html" => "text/html; charset=utf-8",
            "js" | "mjs" => "application/javascript; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "svg" => "image/svg+xml",
            "json" => "application/json",
            "ico" => "image/x-icon",
            "png" => "image/png",
            "woff2" => "font/woff2",
            "txt" => "text/plain; charset=utf-8",
            _ => "application/octet-stream",
        }
    }

    fn fnv1a(data: &[u8]) -> u64 {
        data.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }

    // (published name, path of the bytes to embed, gzip?)
    let mut files: Vec<(String, PathBuf, bool)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dist) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
                continue;
            };
            println!("cargo:rerun-if-changed={}", path.display());
            if let Some(plain) = name.strip_suffix(".gz") {
                files.push((plain.to_owned(), path.clone(), true));
            } else if !dist.join(format!("{name}.gz")).exists() && path.is_file() {
                files.push((name, path.clone(), false));
            }
        }
    }
    files.sort();

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    if !files.iter().any(|(n, _, _)| n == "index.html") {
        println!(
            "cargo:warning=web/dist/index.html not found - embedding a placeholder page. Run `npm ci && npm run build` in web/."
        );
        let placeholder = out_dir.join("placeholder.html");
        std::fs::write(
            &placeholder,
            "<!doctype html><meta charset=utf-8><title>hwhack</title>\
             <p>Web UI not embedded. Run <code>npm ci &amp;&amp; npm run build</code> in <code>web/</code> and rebuild the firmware.",
        )
        .unwrap();
        files.push(("index.html".to_owned(), placeholder, false));
    }

    let mut total = 0usize;
    let mut code = String::from("static ASSETS: &[(&str, Asset)] = &[\n");
    for (name, path, gzip) in &files {
        let bytes = std::fs::read(path).unwrap();
        total += bytes.len();
        let abs = std::fs::canonicalize(path).unwrap();
        writeln!(
            code,
            "    ({name:?}, Asset {{ mime: {:?}, gzip: {gzip}, etag: \"\\\"{:016x}\\\"\", body: include_bytes!({:?}) }}),",
            mime(name),
            fnv1a(&bytes),
            abs.display().to_string(),
        )
        .unwrap();
    }
    code.push_str("];\n");
    std::fs::write(out_dir.join("web_assets.rs"), code).unwrap();

    if total > 2 * 1024 * 1024 {
        println!(
            "cargo:warning=embedded web assets are {} KiB; the firmware image must fit the app partition (~4 MB with espflash defaults on 4 MB flash)",
            total / 1024
        );
    }
}
