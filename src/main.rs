use macroquad::prelude::*;
use macroquad::window::Conf;
use serde::{Deserialize, Serialize};
use std::net::UdpSocket;

const WIDTH: f32 = 800.0;
const HEIGHT: f32 = 480.0;
const PADDLE_RADIUS: f32 = 18.0;
const PUCK_RADIUS: f32 = 12.0;
const PADDLE_SPEED: f32 = 300.0;
const TICK_DT: f32 = 1.0 / 60.0;
const GOAL_HEIGHT: f32 = 160.0;
const BORDER_THICKNESS: f32 = 6.0;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
struct PaddleState {
    x: f32,
    y: f32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
struct PuckState {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
}

#[derive(Serialize, Deserialize, Debug)]
enum Packet {
    Hello,               // handshake
    Input(PaddleState),  // client's paddle
    State {              // authoritative state from host
        paddle: PaddleState,
        opponent: PaddleState,
        puck: PuckState,
        score_left: i32,
        score_right: i32,
        game_over: bool,
    },
}

fn clamp(v: f32, a: f32, b: f32) -> f32 { v.min(b).max(a) }

fn window_conf() -> Conf {
    Conf {
        window_title: "Minah Air Hockey".to_string(),
        window_width: WIDTH as i32,
        window_height: HEIGHT as i32,
        ..Conf::default()
    }
}

#[macroquad::main(window_conf = "window_conf")]
async fn main() {
    // Ignore any command-line arguments — role is chosen via in-game menu
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        println!("Command-line arguments detected but ignored. Use the in-game menu to select Host or Connect.");
    }
    // UI state: start on the menu instead of parsing CLI args
    enum Screen { Menu, Playing }
    let mut screen = Screen::Menu;
    let mut input_ip = String::from("127.0.0.1:3456");
    let mut input_active = false;
    let mut input_just_activated = false;
    let mut role_host = false;
    let mut peer_addr = String::new();
    let mut socket_opt: Option<UdpSocket> = None;
    let mut menu_sel: usize = 0; // 0 = Host, 1 = Connect

    // initial states
    let mut my_paddle = if role_host { PaddleState { x: WIDTH * 0.15, y: HEIGHT / 2.0 } } else { PaddleState { x: WIDTH * 0.85, y: HEIGHT / 2.0 } };
    let mut other_paddle = if role_host { PaddleState { x: WIDTH * 0.85, y: HEIGHT / 2.0 } } else { PaddleState { x: WIDTH * 0.15, y: HEIGHT / 2.0 } };
    let mut puck = PuckState { x: WIDTH / 2.0, y: HEIGHT / 2.0, vx: 140.0, vy: 60.0 };
    let mut score_left: i32 = 0;
    let mut score_right: i32 = 0;
    let mut game_over: bool = false;

    let mut known_peer: Option<std::net::SocketAddr> = None;
    let mut recv_count: usize = 0;
    let mut last_recv_time: f64 = 0.0;
    let mut last_hello_time: f64 = 0.0;
    let mut last = get_time();
    loop {
        // MENU UI (keyboard only)
        if let Screen::Menu = screen {
            clear_background(BLACK);
            draw_text("Minah Air Hockey", WIDTH/2.0 - 220.0, 100.0, 64.0, WHITE);

            let host_rect = Rect::new(WIDTH/2.0 - 150.0, 170.0, 300.0, 60.0);
            let conn_rect = Rect::new(WIDTH/2.0 - 150.0, 250.0, 300.0, 60.0);
            let input_rect = Rect::new(WIDTH/2.0 - 200.0, 330.0, 400.0, 40.0);

            // keyboard navigation: W/S or Up/Down to change selection
            if is_key_pressed(KeyCode::S) || is_key_pressed(KeyCode::Down) {
                menu_sel = (menu_sel + 1) % 2;
            }
            if is_key_pressed(KeyCode::W) || is_key_pressed(KeyCode::Up) {
                menu_sel = (menu_sel + 2 - 1) % 2;
            }

            // draw with selection highlight
            let host_color = if menu_sel == 0 { LIGHTGRAY } else { DARKGRAY };
            let conn_color = if menu_sel == 1 { LIGHTGRAY } else { DARKGRAY };
            draw_rectangle(host_rect.x, host_rect.y, host_rect.w, host_rect.h, host_color);
            draw_text("Host Play", host_rect.x + 32.0, host_rect.y + 40.0, 32.0, WHITE);
            draw_rectangle(conn_rect.x, conn_rect.y, conn_rect.w, conn_rect.h, conn_color);
            draw_text("Connect", conn_rect.x + 32.0, conn_rect.y + 40.0, 32.0, WHITE);

            // input box under connect
            draw_rectangle(input_rect.x, input_rect.y, input_rect.w, input_rect.h, LIGHTGRAY);
            draw_text(&input_ip, input_rect.x + 8.0, input_rect.y + 28.0, 22.0, BLACK);

            // Enter to select or to focus input
            if is_key_pressed(KeyCode::Enter) {
                if menu_sel == 0 {
                    // start as host
                    role_host = true;
                    let bind_addr = "0.0.0.0:3456";
                    let mut bound = false;
                    match UdpSocket::bind(bind_addr) {
                        Ok(s) => {
                            s.set_nonblocking(true).ok();
                            if let Ok(local) = s.local_addr() { println!("Host bound to {}", local); }
                            socket_opt = Some(s);
                            bound = true;
                        }
                        Err(e) => {
                            println!("failed to bind socket {}: {}", bind_addr, e);
                            // remain on menu so user can retry or choose another action
                            role_host = false;
                        }
                    }
                    if bound {
                        // initialize host positions and puck when entering play
                        my_paddle = PaddleState { x: WIDTH * 0.15, y: HEIGHT / 2.0 };
                        other_paddle = PaddleState { x: WIDTH * 0.85, y: HEIGHT / 2.0 };
                        puck = PuckState { x: WIDTH / 2.0, y: HEIGHT / 2.0, vx: 140.0, vy: 60.0 };
                        score_left = 0; score_right = 0; game_over = false;
                        println!("Starting as host");
                        screen = Screen::Playing;
                    }
                } else {
                    // focus input for connect (ignore the Enter that activated the input)
                    input_active = true;
                    input_just_activated = true;
                    // consume this frame so the Enter that opened the input cannot immediately submit
                    next_frame().await;
                    continue;
                }
            }

            // keyboard input for IP field
            if input_active {
                if let Some(c) = get_char_pressed() {
                    let bytes = c.to_string().into_bytes();
                    println!("get_char_pressed -> {:?} (U+{:04X}) bytes {:?}", c, c as u32, bytes);
                    // treat CR/LF as Enter submit when input is focused
                    if (c == '\r' || c == '\n') {
                        println!("Char-based Enter detected, submitting input='{}'", input_ip);
                        if !input_ip.is_empty() {
                            peer_addr = input_ip.clone();
                            let sock = UdpSocket::bind("0.0.0.0:0").unwrap_or_else(|e| panic!("failed to bind client socket: {}", e));
                            sock.set_nonblocking(true).ok();
                            let peer = peer_addr.parse::<std::net::SocketAddr>().ok();
                            if peer.is_none() {
                                println!("Failed to parse peer_addr='{}'", peer_addr);
                            }
                            if let Ok(local) = sock.local_addr() { println!("Client bound from {}", local); }
                            if let Some(p) = peer {
                                let data = bincode::serialize(&Packet::Hello).unwrap();
                                match sock.send_to(&data, p) {
                                    Ok(n) => println!("Sent Hello ({} bytes) to {}", n, p),
                                    Err(e) => println!("Failed to send Hello to {}: {}", p, e),
                                }
                                match sock.send_to(b"DEBUG_HELLO", p) {
                                    Ok(n) => println!("Sent DEBUG_HELLO ({} bytes) to {}", n, p),
                                    Err(e) => println!("Failed to send DEBUG_HELLO to {}: {}", p, e),
                                }
                            }
                            known_peer = None;
                            last_hello_time = 0.0;
                            socket_opt = Some(sock);
                            role_host = false;
                            my_paddle = PaddleState { x: WIDTH * 0.85, y: HEIGHT / 2.0 };
                            other_paddle = PaddleState { x: WIDTH * 0.15, y: HEIGHT / 2.0 };
                            puck = PuckState { x: WIDTH / 2.0, y: HEIGHT / 2.0, vx: 140.0, vy: 60.0 };
                            score_left = 0; score_right = 0; game_over = false;
                            println!("Starting as client -> {}", peer_addr);
                            screen = Screen::Playing;
                        }
                        input_just_activated = false;
                    } else if c.is_control() {
                        println!("Ignored control char U+{:04X}", c as u32);
                    } else {
                        input_ip.push(c);
                    }
                    input_just_activated = false;
                }
                if is_key_pressed(KeyCode::Backspace) {
                    input_just_activated = false;
                    println!("Backspace pressed (input_active={})", input_active);
                    input_ip.pop();
                }
                if is_key_pressed(KeyCode::Delete) {
                    input_just_activated = false;
                    println!("Delete pressed (input_active={})", input_active);
                    input_ip.pop();
                }
                // Enter to connect
                if is_key_pressed(KeyCode::Enter) {
                    println!("Key Enter pressed in input (input_just_activated={})", input_just_activated);
                }
                if is_key_pressed(KeyCode::Enter) && !input_just_activated {
                    println!("Submitting connect with input='{}'", input_ip);
                    if !input_ip.is_empty() {
                        peer_addr = input_ip.clone();
                        let sock = UdpSocket::bind("0.0.0.0:0").unwrap_or_else(|e| panic!("failed to bind client socket: {}", e));
                        sock.set_nonblocking(true).ok();
                        // known_peer = peer_addr.parse().ok();
                        let peer = peer_addr.parse::<std::net::SocketAddr>().ok();
                        if peer.is_none() {
                            println!("Failed to parse peer_addr='{}'", peer_addr);
                        }
                        if let Ok(local) = sock.local_addr() { println!("Client bound from {}", local); }
                        if let Some(p) = peer {
                            let data = bincode::serialize(&Packet::Hello).unwrap();
                            match sock.send_to(&data, p) {
                                Ok(n) => println!("Sent Hello ({} bytes) to {}", n, p),
                                Err(e) => println!("Failed to send Hello to {}: {}", p, e),
                            }
                            // also send a plain-text debug packet to help capture with tcpdump
                            match sock.send_to(b"DEBUG_HELLO", p) {
                                Ok(n) => println!("Sent DEBUG_HELLO ({} bytes) to {}", n, p),
                                Err(e) => println!("Failed to send DEBUG_HELLO to {}: {}", p, e),
                            }
                        }
                        known_peer = None;
                        // force periodic hello to send immediately
                        last_hello_time = 0.0;
                        socket_opt = Some(sock);
                        role_host = false;
                        // client-side initial positions
                        my_paddle = PaddleState { x: WIDTH * 0.85, y: HEIGHT / 2.0 };
                        other_paddle = PaddleState { x: WIDTH * 0.15, y: HEIGHT / 2.0 };
                        puck = PuckState { x: WIDTH / 2.0, y: HEIGHT / 2.0, vx: 140.0, vy: 60.0 };
                        score_left = 0; score_right = 0; game_over = false;
                        println!("Starting as client -> {}", peer_addr);
                        screen = Screen::Playing;
                    }
                    input_just_activated = false;
                }
                if is_key_pressed(KeyCode::Escape) {
                    input_active = false;
                    input_just_activated = false;
                }
            }

            next_frame().await;
            continue;
        }

        let now = get_time();
        let mut dt = (now - last) as f32;
        if dt > 0.05 { dt = 0.05; }
        last = now;

        // handle input (WASD)
        let mut dx: f32 = 0.0;
        let mut dy: f32 = 0.0;
        if !game_over {
            if is_key_down(KeyCode::W) { dy -= 1.0; }
            if is_key_down(KeyCode::S) { dy += 1.0; }
            if is_key_down(KeyCode::A) { dx -= 1.0; }
            if is_key_down(KeyCode::D) { dx += 1.0; }
        }
        let len = (dx*dx + dy*dy).sqrt();
        if len > 0.0 {
            dx /= len; dy /= len;
        }
        my_paddle.x += dx * PADDLE_SPEED * dt;
        my_paddle.y += dy * PADDLE_SPEED * dt;

        // clamp paddles to their half
        if role_host {
            my_paddle.x = clamp(my_paddle.x, 0.0 + PADDLE_RADIUS, WIDTH / 2.0 - PADDLE_RADIUS);
            other_paddle.x = clamp(other_paddle.x, WIDTH / 2.0 + PADDLE_RADIUS, WIDTH - PADDLE_RADIUS);
        } else {
            my_paddle.x = clamp(my_paddle.x, WIDTH / 2.0 + PADDLE_RADIUS, WIDTH - PADDLE_RADIUS);
            other_paddle.x = clamp(other_paddle.x, 0.0 + PADDLE_RADIUS, WIDTH / 2.0 - PADDLE_RADIUS);
        }
        my_paddle.y = clamp(my_paddle.y, PADDLE_RADIUS, HEIGHT - PADDLE_RADIUS);

        // Networking: use created socket
        let socket = socket_opt.as_ref().expect("socket not created");

        // Networking: receive all pending packets
        let mut buf = [0u8; 1024];
        loop {
            match socket.recv_from(&mut buf) {
                Ok((n, addr)) => {
                    println!("Recv {} bytes from {}", n, addr);
                    // if known_peer.is_none() {
                    //     known_peer = Some(addr);
                    // }
                    match bincode::deserialize::<Packet>(&buf[..n]) {
                        Ok(pkt) => {
                            recv_count += 1;
                            last_recv_time = get_time();
                            match pkt {
                                Packet::Hello => {
                                    // handshake
                                    known_peer = Some(addr);
                                    println!("Hello from {}", addr);
                                    // reply immediately with authoritative state so client has initial game state
                                    let state_pkt = Packet::State { paddle: my_paddle, opponent: other_paddle, puck, score_left, score_right, game_over };
                                    if let Ok(data) = bincode::serialize(&state_pkt) {
                                        match socket.send_to(&data, addr) {
                                            Ok(n) => println!("Sent State ({} bytes) to {}", n, addr),
                                            Err(e) => println!("Failed to send State to {}: {}", addr, e),
                                        }
                                    }
                                }
                                Packet::Input(p) => {
                                    // if host, client input becomes other_paddle
                                    if role_host {
                                        println!("Input from {} -> {:?}", addr, p);
                                        other_paddle = p;
                                    }
                                }
                                Packet::State { paddle, opponent, puck: p, score_left: sl, score_right: sr, game_over: go } => {
                                    // client receives authoritative state
                                    if !role_host {
                                        known_peer = Some(addr);
                                        // `paddle` is host's paddle; show it as the opponent on client
                                        other_paddle = paddle;
                                        // optionally correct local paddle to host-observed value
                                        // my_paddle = opponent;
                                        puck = p;
                                        score_left = sl;
                                        score_right = sr;
                                        game_over = go;
                                        println!("State from {} -> host_paddle={:?}, client_seen={:?}, scores={}:{} game_over={}", addr, paddle, opponent, sl, sr, go);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            println!("Failed to deserialize packet from {}: {}", addr, e);
                            // dump raw bytes for inspection
                            if n > 0 {
                                let s = match std::str::from_utf8(&buf[..n]) {
                                    Ok(text) => format!("as utf8: '{}'", text),
                                    Err(_) => format!("bytes: {:?}", &buf[..n]),
                                };
                                println!("Raw recv {}: {}", n, s);
                            }
                        }
                    }
                }
                Err(_e) => {
                    // WouldBlock expected when no more packets
                    break;
                }
            }
        }

        let now_t = get_time();

        if role_host && known_peer.is_none() && now_t - last_hello_time > 0.5 {
            println!("Host waiting for client...");
            last_hello_time = now_t;
        }
        // Host authoritative physics update
        if role_host && !game_over {
            // update puck
            puck.x += puck.vx * dt;
            puck.y += puck.vy * dt;

            // wall collisions
            if puck.y < PUCK_RADIUS { puck.y = PUCK_RADIUS; puck.vy = -puck.vy; }
            if puck.y > HEIGHT - PUCK_RADIUS { puck.y = HEIGHT - PUCK_RADIUS; puck.vy = -puck.vy; }

            // paddle collisions simple circle-vs-circle
            let coll = |paddle: &PaddleState, puck: &mut PuckState| {
                let dx = puck.x - paddle.x;
                let dy = puck.y - paddle.y;
                let dist2 = dx*dx + dy*dy;
                let min_dist = PADDLE_RADIUS + PUCK_RADIUS;
                if dist2 < min_dist*min_dist {
                    let dist = dist2.sqrt().max(0.001);
                    // push puck out
                    let nx = dx / dist;
                    let ny = dy / dist;
                    puck.x = paddle.x + nx * min_dist;
                    puck.y = paddle.y + ny * min_dist;
                    // reflect velocity with some addition from paddle movement
                    let rel_vx = puck.vx + (paddle.x - paddle.x) * 0.0;
                    let dot = rel_vx * nx + puck.vy * ny;
                    puck.vx = puck.vx - 2.0 * dot * nx;
                    puck.vy = puck.vy - 2.0 * dot * ny;
                    // add a bit of speed
                    puck.vx *= 1.05; puck.vy *= 1.05;
                }
            };

            coll(&my_paddle, &mut puck);
            coll(&other_paddle, &mut puck);

            // horizontal edges: goal detection with opening in the borders
            let goal_top = (HEIGHT - GOAL_HEIGHT) / 2.0;
            let goal_bottom = goal_top + GOAL_HEIGHT;

            // left edge
            if puck.x - PUCK_RADIUS < 0.0 {
                if puck.y >= goal_top && puck.y <= goal_bottom {
                    // right scores
                    score_right += 1;
                    puck.x = WIDTH / 2.0; puck.y = HEIGHT / 2.0; puck.vx = 160.0; puck.vy = 30.0;
                    if score_right > 12 { game_over = true; }
                } else {
                    puck.x = PUCK_RADIUS; puck.vx = -puck.vx;
                }
            }

            // right edge
            if puck.x + PUCK_RADIUS > WIDTH {
                if puck.y >= goal_top && puck.y <= goal_bottom {
                    // left scores
                    score_left += 1;
                    puck.x = WIDTH / 2.0; puck.y = HEIGHT / 2.0; puck.vx = -160.0; puck.vy = -30.0;
                    if score_left > 12 { game_over = true; }
                } else {
                    puck.x = WIDTH - PUCK_RADIUS; puck.vx = -puck.vx;
                }
            }

            // send authoritative state to peer if known
            if let Some(peer) = known_peer {
                let state_pkt = Packet::State { paddle: my_paddle, opponent: other_paddle, puck, score_left, score_right, game_over };
                if let Ok(data) = bincode::serialize(&state_pkt) {
                    match socket.send_to(&data, peer) {
                        Ok(n) => println!("Sent State ({} bytes) to {}", n, peer),
                        Err(e) => println!("Failed to send State to {}: {}", peer, e),
                    }
                }
            }
        } else {
            // client: send input to host
            // allow host to restart with R key
            if game_over && role_host && is_key_pressed(KeyCode::R) {
                // reset scores and puck
                score_left = 0; score_right = 0; game_over = false;
                puck.x = WIDTH/2.0; puck.y = HEIGHT/2.0; puck.vx = 140.0; puck.vy = 60.0;
                // send immediate state
                if let Some(peer) = known_peer {
                    let state_pkt = Packet::State { paddle: my_paddle, opponent: other_paddle, puck, score_left, score_right, game_over };
                    if let Ok(data) = bincode::serialize(&state_pkt) {
                        let _ = socket.send_to(&data, peer);
                    }
                }
            }
            // periodic hello retransmit until host responds
            let now_t = get_time();
            if known_peer.is_none() && !peer_addr.is_empty() && now_t - last_hello_time > 0.5 {
                if let Ok(peer) = peer_addr.parse::<std::net::SocketAddr>() {
                    println!("Attempting periodic hello to {}", peer);
                    if let Ok(data) = bincode::serialize(&Packet::Hello) {
                        match socket.send_to(&data, peer) {
                            Ok(n) => println!("Periodic Hello sent ({} bytes) to {}", n, peer),
                            Err(e) => println!("Periodic Hello failed to {}: {}", peer, e),
                        }
                    }
                    // also send a plain-text debug packet
                    match socket.send_to(b"DEBUG_HELLO", peer) {
                        Ok(n) => println!("Periodic DEBUG_HELLO sent ({} bytes) to {}", n, peer),
                        Err(e) => println!("Periodic DEBUG_HELLO failed to {}: {}", peer, e),
                    }
                    last_hello_time = now_t;
                }
            }

            // extra aggressive debug send if still no peer (helps on some loopback configs)
            if known_peer.is_none() && !peer_addr.is_empty() {
                if let Ok(peer) = peer_addr.parse::<std::net::SocketAddr>() {
                    if let Err(e) = socket.send_to(b"FORCE_DEBUG_HELLO", peer) {
                        // don't spam too much in logs
                    } else {
                        println!("FORCE_DEBUG_HELLO sent to {}", peer);
                    }
                }
            }

            if let Some(peer) = known_peer {
                if let Ok(data) = bincode::serialize(&Packet::Input(my_paddle)) {
                    if let Err(e) = socket.send_to(&data, peer) {
                        println!("Failed to send Input to {}: {}", peer, e);
                    }
                }
            }
        }

        // rendering
        clear_background(BLACK);
        // draw arena borders with goal openings
        let goal_top = (HEIGHT - GOAL_HEIGHT) / 2.0;
        let goal_bottom = goal_top + GOAL_HEIGHT;

        // top border
        draw_line(0.0, BORDER_THICKNESS/2.0, WIDTH, BORDER_THICKNESS/2.0, BORDER_THICKNESS, GRAY);
        // bottom border
        draw_line(0.0, HEIGHT - BORDER_THICKNESS/2.0, WIDTH, HEIGHT - BORDER_THICKNESS/2.0, BORDER_THICKNESS, GRAY);

        // left border split around goal
        draw_line(BORDER_THICKNESS/2.0, 0.0, BORDER_THICKNESS/2.0, goal_top, BORDER_THICKNESS, GRAY);
        draw_line(BORDER_THICKNESS/2.0, goal_bottom, BORDER_THICKNESS/2.0, HEIGHT, BORDER_THICKNESS, GRAY);
        // right border split
        draw_line(WIDTH - BORDER_THICKNESS/2.0, 0.0, WIDTH - BORDER_THICKNESS/2.0, goal_top, BORDER_THICKNESS, GRAY);
        draw_line(WIDTH - BORDER_THICKNESS/2.0, goal_bottom, WIDTH - BORDER_THICKNESS/2.0, HEIGHT, BORDER_THICKNESS, GRAY);

        // goal caps (visual)
        draw_rectangle(0.0, goal_top - 6.0, BORDER_THICKNESS * 2.0, 6.0, DARKGRAY);
        draw_rectangle(0.0, goal_bottom, BORDER_THICKNESS * 2.0, 6.0, DARKGRAY);
        draw_rectangle(WIDTH - BORDER_THICKNESS*2.0, goal_top - 6.0, BORDER_THICKNESS * 2.0, 6.0, DARKGRAY);
        draw_rectangle(WIDTH - BORDER_THICKNESS*2.0, goal_bottom, BORDER_THICKNESS * 2.0, 6.0, DARKGRAY);

        // center line
        draw_line(WIDTH/2.0, 0.0, WIDTH/2.0, HEIGHT, 2.0, GRAY);

        // draw paddles and puck
        draw_circle(my_paddle.x, my_paddle.y, PADDLE_RADIUS, BLUE);
        draw_circle(other_paddle.x, other_paddle.y, PADDLE_RADIUS, RED);
        draw_circle(puck.x, puck.y, PUCK_RADIUS, WHITE);

        // scores
        let score_text = format!("{}  -  {}", score_left, score_right);
        draw_text(&score_text, WIDTH/2.0 - 40.0, 36.0, 32.0, WHITE);

        // winner overlay
        if game_over {
            let winner_role = if score_left > score_right { "Host" } else { "Client" };
            // dark overlay
            draw_rectangle(0.0, 0.0, WIDTH, HEIGHT, Color::new(0.0, 0.0, 0.0, 0.6));
            let title = format!("{} Wins!", winner_role);
            draw_text(&title, WIDTH/2.0 - 120.0, HEIGHT/2.0 - 10.0, 48.0, GOLD);
            // role-specific message
            let you_text = if (winner_role == "Host" && role_host) || (winner_role == "Client" && !role_host) { "You Win" } else { "You Lose" };
            draw_text(you_text, WIDTH/2.0 - 40.0, HEIGHT/2.0 + 40.0, 32.0, WHITE);
            draw_text("Press R to restart (host only)", WIDTH/2.0 - 160.0, HEIGHT/2.0 + 80.0, 20.0, YELLOW);
        }

        // HUD
        let role_text = if role_host { "Host (authoritative) - WASD to move" } else { "Client - WASD to move" };
        draw_text(role_text, 12.0, 20.0, 22.0, WHITE);
        if !role_host {
            draw_text("Run: cargo run -- connect <host_ip:3456>", 12.0, 44.0, 18.0, WHITE);
        } else {
            draw_text("Run: cargo run -- host (binds 0.0.0.0:3456)", 12.0, 44.0, 18.0, WHITE);
        }

        // debug: peer and packet info
        let peer_text = if let Some(p) = known_peer { format!("Peer: {}", p) } else { "Peer: (none)".to_string() };
        draw_text(&peer_text, 12.0, 74.0, 18.0, YELLOW);
        let recv_text = format!("Recv pkts: {}  last: {:.2}s ago", recv_count, get_time() - last_recv_time);
        draw_text(&recv_text, 12.0, 96.0, 18.0, YELLOW);

        // diagnostics: press I to print socket and peer info to the console
        if is_key_pressed(KeyCode::I) {
            if let Some(s) = socket_opt.as_ref() {
                match s.local_addr() {
                    Ok(local) => println!("Socket local address: {}", local),
                    Err(e) => println!("Socket local address: <error>: {}", e),
                }
            } else {
                println!("Socket not created yet");
            }
            println!("Configured peer_addr: '{}', known_peer: {:?}", peer_addr, known_peer);
        }

        // present and wait next frame
        next_frame().await;
    }
}