use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let mut headers = vec![];
    let mut sources = vec![];
    for entry in fs::read_dir("protocols").unwrap() {
        let path = entry.unwrap().path();

        let xml_path = path.to_str().unwrap();

        let path = path.with_extension("h");
        let filename = path.file_name().unwrap();
        let header_path = out_dir.join(filename);

        let path = path.with_extension("c");
        let filename = path.file_name().unwrap();
        let source_path = out_dir.join(filename);

        Command::new("wayland-scanner")
            .args(["client-header", xml_path, header_path.to_str().unwrap()])
            .status()
            .unwrap();
        Command::new("wayland-scanner")
            .args(["private-code", xml_path, source_path.to_str().unwrap()])
            .status()
            .unwrap();
        headers.push(header_path.to_str().unwrap().to_string());
        sources.push(source_path.to_str().unwrap().to_string());
    }

    let bindings = bindgen::Builder::default()
        .derive_copy(false)
        .new_type_alias("wl_fixed_t")
        .prepend_enum_name(false)
        .wrap_static_fns(true)
        .headers(headers)
        .blocklist_item("FP_INT_UPWARD")
        .blocklist_item("FP_INT_DOWNWARD")
        .blocklist_item("FP_INT_TOWARDZERO")
        .blocklist_item("FP_INT_TONEARESTFROMZERO")
        .blocklist_item("FP_INT_TONEAREST")
        .blocklist_item("FP_NAN")
        .blocklist_item("FP_INFINITE")
        .blocklist_item("FP_ZERO")
        .blocklist_item("FP_SUBNORMAL")
        .blocklist_item("FP_NORMAL")
        .generate()
        .unwrap();

    bindings.write_to_file(out_dir.join("bindings.rs")).unwrap();

    let wrapper = env::temp_dir().join("bindgen").join("extern.c");
    cc::Build::new()
        .file(&wrapper)
        .files(sources)
        .compile("extern");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-link-lib=dylib=wayland-client");
    println!("cargo:rustc-link-lib=static=extern");
}
