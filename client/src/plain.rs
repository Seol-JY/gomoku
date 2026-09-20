//! Line-oriented front end for pipes and debugging

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::driver::{Driver, Exit};
use crate::session::{HELP, LogEntry};
use crate::tui::board;

fn flush_log(driver: &Driver, printed: &mut usize) {
    for entry in &driver.session.log[*printed..] {
        match entry {
            LogEntry::Chat { from, text, .. } => println!("<{from}> {text}"),
            LogEntry::Notice(t) => println!("* {t}"),
            LogEntry::Error(t) => println!("! {t}"),
        }
    }
    *printed = driver.session.log.len();
}

fn print_board(driver: &Driver) {
    println!();
    for row in board::text_rows(driver.session.snapshot.as_ref(), false) {
        println!("{row}");
    }
    println!(
        "-- {} [{}]",
        driver.session.turn_text(),
        driver.session.connection.label()
    );
}

pub async fn run(mut driver: Driver) -> Result<Exit> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut printed = 0usize;
    println!("gomoku (plain mode). Type /help for commands.");
    for l in HELP {
        println!("  {l}");
    }
    flush_log(&driver, &mut printed);
    print_board(&driver);
    let mut last_seen = driver.session.snapshot.as_ref().map(|s| (s.seq, s.status));
    let mut stdin_open = true;

    loop {
        tokio::select! {
            line = lines.next_line(), if stdin_open => match line {
                Ok(Some(l)) => {
                    let action = driver.session.interpret(&l);
                    driver.perform(action);
                    flush_log(&driver, &mut printed);
                }
                Ok(None) => {
                    stdin_open = false;
                    println!("* stdin closed; still following the game (Ctrl+C to quit)");
                }
                Err(e) => return Err(e.into()),
            },
            msg = driver.rx.recv() => match msg {
                Some(m) => {
                    driver.handle(m);
                    flush_log(&driver, &mut printed);
                    let now = driver.session.snapshot.as_ref().map(|s| (s.seq, s.status));
                    if now != last_seen {
                        last_seen = now;
                        print_board(&driver);
                    }
                }
                None => return Ok(Exit::Fatal("event channel closed".into())),
            },
        }
        if let Some(exit) = driver.exit.clone() {
            driver.shutdown();
            return Ok(exit);
        }
    }
}
