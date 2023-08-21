// use std::{
//     io::{self, Write},
//     time::Duration,
// };

// use minifb::{Window, WindowOptions};

use std::{future, path::PathBuf, pin::Pin};

use clap::Parser;
use dom::Dom;
use html::ParseEvent;
use io::AsyncStrReader;
use smol::fs::File;

mod asyncro;
mod dom;
mod html;
mod io;
mod tls;
mod uri;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// An optional document location
    location: Option<PathBuf>, // TODO: Uri
}

fn main() -> std::io::Result<()> {
    let args = Args::parse();
    let mut dom = Dom::new();
    smol::block_on(async {
        let file = File::open(args.location.unwrap()).await?;
        let reader = AsyncStrReader::new(file);
        let mut parser = html::Parser::new(reader);
        loop {
            match future::poll_fn(|cx| Pin::new(&mut parser).poll_next(cx, &mut dom)).await {
                ParseEvent::Done => break,
                ParseEvent::Fatal(_, err) => {
                    return Err(std::io::Error::new(std::io::ErrorKind::Other, err))
                }
                _ => {}
            }
        }
        dom.write_tree(&mut std::io::stdout())?;
        Ok(())
    })
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
