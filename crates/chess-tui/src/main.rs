use std::time::{Duration, Instant};

use chess_net::connect::Servers;
use chess_tui::{app::App, net::Net, render::Renderer};
use ratatui::crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute, terminal,
};

/// Redraw interval; drives the clock, the cursor pulse and piece slides.
const FRAME: Duration = Duration::from_millis(33);

const USAGE: &str = "\
usage: chess-tui [--server HOST[:PORT]] [--relay HOST[:PORT]] [--force-relay]

  --server   rendezvous server for peer-to-peer games: $CHESS_RENDEZVOUS,
             or 127.0.0.1; port 7777
  --relay    relay: $CHESS_RELAY, or the server's host on port 443
  --force-relay  skip the direct connection (both players must pass it)";

fn main() -> std::io::Result<()> {
    let mut servers = Servers::from_env();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--server" => servers.rendezvous = args.next().unwrap_or_default(),
            "--relay" => servers.relay = args.next(),
            "--force-relay" => servers.force_relay = true,
            _ => {
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }

    let mut app = App::new(Instant::now());
    app.server = servers.rendezvous.clone();
    let mut net = Net::new(servers)?;
    app.hires = chess_tui::octants_supported();
    let mut terminal = ratatui::init();
    // Taking the mouse also stops the terminal selecting text on a click or
    // drag. Give it back on a panic too, or the shell fills with mouse codes.
    execute!(std::io::stdout(), EnableMouseCapture)?;
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        _ = execute!(std::io::stdout(), DisableMouseCapture);
        hook(info);
    }));
    if let Ok(size) = terminal::size() {
        app.size = size;
    }
    let mut renderer = Renderer::new(chess_tui::truecolor());
    let result = (|| {
        while !app.quit {
            for event in net.poll() {
                app.on_net(event, Instant::now());
            }
            let now = Instant::now();
            app.tick(now);
            for request in app.requests.drain(..) {
                net.request(request);
            }
            terminal.draw(|f| renderer.draw(f, &app, now))?;
            if event::poll(FRAME)? {
                match event::read()? {
                    Event::Key(key) => app.on_key(key, Instant::now()),
                    Event::Mouse(mouse) => app.on_mouse(mouse, Instant::now()),
                    Event::Resize(cols, rows) => app.size = (cols, rows),
                    _ => {}
                }
            }
            for request in app.requests.drain(..) {
                net.request(request);
            }
            if let Some(text) = app.clipboard.take() {
                app.copied = Some((Instant::now(), chess_tui::clipboard::copy(&text)));
            }
        }
        Ok(())
    })();
    _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}
