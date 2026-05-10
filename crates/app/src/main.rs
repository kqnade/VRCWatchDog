// 起動本体は lib.rs の run() に集約。bin はそこに委譲するだけ。
fn main() {
    vrcwatchdog_app_lib::run();
}
