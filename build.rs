use std::{env, path::PathBuf, process::Command};

fn main() {
    let manifest = "assets/gnune.gresource.xml";
    println!("cargo:rerun-if-changed={manifest}");
    println!("cargo:rerun-if-changed=assets/gnuneIconDraft2.png");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("gnune.gresource");
    let status = Command::new("glib-compile-resources")
        .args([manifest, "--sourcedir=assets", "--target"])
        .arg(output)
        .status()
        .expect("GTK development tools must include glib-compile-resources");
    assert!(status.success(), "could not embed the application icon");
}
