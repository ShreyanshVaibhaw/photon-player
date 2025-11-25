fn main() {
    // 1. Compile the Slint UI
    slint_build::compile("src/app.slint").unwrap();

    // 2. Link MPV
    let mpv_path = r"C:\mpv-dev"; 

    println!("cargo:rustc-link-search=native={}", mpv_path);
    
    // CHANGE: We now look for 'mpv', which matches 'mpv.lib'
    println!("cargo:rustc-link-lib=mpv"); 
}