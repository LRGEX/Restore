// L-5.2 verification: minimal window with Slint 1.17 + renderer-software.
// If this renders (screenshot shows text on solid background), the renderer
// upgrade is visually confirmed. Auto-exits after 8 seconds.
slint::slint! {
    export component MainWindow inherits Window {
        width: 420px;
        height: 180px;
        background: #1e1e1e;
        VerticalLayout {
            alignment: center;
            spacing: 10px;
            Rectangle { background: #2d5a88; height: 70px; width: 320px; }
            Text {
                text: "1.5.2 RENDER TEST — solid background + blue bar = rendering works";
                color: white;
                font-size: 14px;
                horizontal-alignment: center;
            }
        }
    }
}

fn main() {
    let window = MainWindow::new().expect("create window");
    window.show();
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(9));
        std::process::exit(0);
    });
    slint::run_event_loop().expect("event loop");
}
