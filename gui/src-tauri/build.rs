fn main() {
    // 前端产物变化时必须重新编译，否则 cargo 会跳过，exe 里还是旧页面。
    println!("cargo:rerun-if-changed=../dist");
    tauri_build::build()
}
