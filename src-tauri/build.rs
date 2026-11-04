// 强制重跑：tauri_build 在前端 dist 尚未生成时首次运行不会登记 rerun-if-changed，
// 导致后续 dist 更新不触发资源重嵌（release 白屏）。dist 就绪后需重跑一次登记。
fn main() {
    tauri_build::build()
}
