#![feature(poll_ready)]

// use std::{
//     io::{self, Write},
//     time::Duration,
// };

// use minifb::{Window, WindowOptions};

mod dom;
mod html;
mod io;

fn main() {
    smol::block_on(async {});
    //    let mut win = Window::new(
    //        "web",
    //        800,
    //        600,
    //        WindowOptions {
    //            resize: true,
    //            ..Default::default()
    //        },
    //    )
    //    .unwrap();
    //
    //    win.limit_update_rate(Some(Duration::from_millis(60)));
    //
    //    while win.is_open() {
    //        win.update();
    //    }
}
