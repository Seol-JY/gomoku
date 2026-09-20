//! End-to-end HTTP + SSE test: real listener, throwaway PostgreSQL database

use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use proto::{
    Ack, ApiError, ChatRequest, Color, CreateRoomResponse, EndReason, ErrorCode, ForbiddenKind,
    Health, JoinByCodeRequest, JoinResponse, MoveRequest, RegisterRequest, RegisterResponse, Role,
    RoomSnapshot, RoomStatus, ServerEvent, StateCause, UpdateMeRequest, UserInfo,
};
use reqwest::StatusCode;
use rules::Pos;
use server::{AppState, Config, build_app, db};
use sqlx::AssertSqlSafe;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

/// One throwaway `gomoku_test_*` database per test, via the admin URL in `GOMOKU_TEST_DATABASE_URL`
struct TestServer {
    base: String,
    pool: sqlx::PgPool,
    admin: PgConnectOptions,
    db_name: String,
}

const DEFAULT_ADMIN_URL: &str = "postgres://gomoku:gomoku@localhost:5432/postgres";

impl TestServer {
    async fn start() -> Self {
        let _ = dotenvy::dotenv();
        let admin_url =
            std::env::var("GOMOKU_TEST_DATABASE_URL").unwrap_or_else(|_| DEFAULT_ADMIN_URL.into());
        let admin: PgConnectOptions = admin_url.parse().expect("valid GOMOKU_TEST_DATABASE_URL");
        let admin_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(admin.clone())
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "cannot reach PostgreSQL at {}: {e}\n\
                     start one with `docker compose up -d db` (or scripts/dev-db.sh) \
                     or point GOMOKU_TEST_DATABASE_URL at an admin database",
                    db::redact(&admin_url)
                )
            });
        let db_name = format!("gomoku_test_{}", uuid_like().replace('-', "_"));
        sqlx::query(AssertSqlSafe(format!("CREATE DATABASE \"{db_name}\"")))
            .execute(&admin_pool)
            .await
            .expect("create test database");
        admin_pool.close().await;

        let pool = db::connect_with(admin.clone().database(&db_name))
            .await
            .expect("db");
        let state = AppState::new(pool.clone(), Config::default());
        let app = build_app(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            base: format!("http://{addr}"),
            pool,
            admin,
            db_name,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// Success path only; a panicking test leaves its database for inspection
    async fn cleanup(self) {
        self.pool.close().await;
        let admin_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(self.admin.clone())
            .await
            .expect("admin connection");
        sqlx::query(AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS \"{}\" WITH (FORCE)",
            self.db_name
        )))
        .execute(&admin_pool)
        .await
        .expect("drop test database");
    }
}

fn uuid_like() -> String {
    uuid::Uuid::now_v7().simple().to_string()
}

struct User {
    http: reqwest::Client,
    token: String,
    id: String,
    name: String,
}

impl User {
    async fn register(srv: &TestServer, name: Option<&str>) -> Self {
        let http = reqwest::Client::new();
        let res: RegisterResponse = http
            .post(srv.url("/users/anonymous"))
            .json(&RegisterRequest {
                display_name: name.map(str::to_owned),
            })
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        Self {
            http,
            token: res.token,
            id: res.user_id,
            name: res.display_name,
        }
    }

    fn post(&self, url: String) -> reqwest::RequestBuilder {
        self.http.post(url).bearer_auth(&self.token)
    }

    fn get(&self, url: String) -> reqwest::RequestBuilder {
        self.http.get(url).bearer_auth(&self.token)
    }

    async fn ok<T: serde::de::DeserializeOwned>(rb: reqwest::RequestBuilder) -> T {
        let res = rb.send().await.unwrap();
        let status = res.status();
        let body = res.text().await.unwrap();
        assert!(
            status.is_success(),
            "expected success, got {status}: {body}"
        );
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("bad json ({e}): {body}"))
    }

    async fn err(rb: reqwest::RequestBuilder) -> (StatusCode, ApiError) {
        let res = rb.send().await.unwrap();
        let status = res.status();
        let body = res.text().await.unwrap();
        assert!(!status.is_success(), "expected error, got {status}: {body}");
        (
            status,
            serde_json::from_str(&body).unwrap_or_else(|e| panic!("bad json ({e}): {body}")),
        )
    }

    async fn play(&self, srv: &TestServer, room: &str, notation: &str, expect_seq: u64) -> Ack {
        let p = Pos::from_notation(notation).unwrap();
        Self::ok(
            self.post(srv.url(&format!("/rooms/{room}/move")))
                .json(&MoveRequest {
                    x: p.x(),
                    y: p.y(),
                    expect_seq,
                }),
        )
        .await
    }

    async fn snapshot(&self, srv: &TestServer, room: &str) -> RoomSnapshot {
        Self::ok(self.get(srv.url(&format!("/rooms/{room}")))).await
    }
}

type SseStream = std::pin::Pin<Box<dyn futures::Stream<Item = (u64, String, ServerEvent)> + Send>>;

async fn open_sse(srv: &TestServer, user: &User, room: &str, last_id: Option<u64>) -> SseStream {
    let mut rb = user
        .get(srv.url(&format!("/rooms/{room}/events")))
        .header("accept", "text/event-stream");
    if let Some(id) = last_id {
        rb = rb.header("last-event-id", id.to_string());
    }
    let res = rb.send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        res.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    assert_eq!(res.headers().get("x-accel-buffering").unwrap(), "no");
    let stream = res
        .bytes_stream()
        .eventsource()
        .filter_map(|item| async move {
            let ev = item.ok()?;
            let parsed: ServerEvent = serde_json::from_str(&ev.data).ok()?;
            Some((ev.id.parse::<u64>().unwrap_or(0), ev.event, parsed))
        });
    Box::pin(stream)
}

async fn next(stream: &mut SseStream) -> (u64, String, ServerEvent) {
    tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("event timeout")
        .expect("stream ended")
}

/// Presence timing nondeterministic
async fn next_non_presence(stream: &mut SseStream) -> (u64, String, ServerEvent) {
    loop {
        let ev = next(stream).await;
        if !matches!(ev.2, ServerEvent::Presence { .. }) {
            return ev;
        }
    }
}

async fn expect_state(stream: &mut SseStream) -> (u64, StateCause, RoomSnapshot) {
    let (seq, kind, ev) = next_non_presence(stream).await;
    match ev {
        ServerEvent::State { cause, snapshot } => {
            assert_eq!(kind, "state");
            (seq, cause, snapshot)
        }
        other => panic!("expected state, got {other:?}"),
    }
}

#[tokio::test]
async fn full_game_flow() {
    let srv = TestServer::start().await;

    // health + version gate
    let health: Health = reqwest::get(srv.url("/healthz"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(health.ok);
    let ready = reqwest::get(srv.url("/readyz")).await.unwrap();
    assert_eq!(ready.status(), StatusCode::OK);
    let res = reqwest::Client::new()
        .get(srv.url("/healthz"))
        .header(proto::CLIENT_VERSION_HEADER, "0.0.1")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UPGRADE_REQUIRED);
    let err: ApiError = res.json().await.unwrap();
    assert_eq!(err.code, ErrorCode::ClientOutdated);
    assert_eq!(
        err.min_client_version.as_deref(),
        Some(server::state::DEFAULT_MIN_CLIENT_VERSION)
    );
    let res = reqwest::Client::new()
        .get(srv.url("/healthz"))
        .header(proto::CLIENT_VERSION_HEADER, env!("CARGO_PKG_VERSION"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // anonymous accounts
    let alice = User::register(&srv, Some("  Alice   Liddell ")).await;
    assert_eq!(alice.name, "Alice Liddell");
    let bob = User::register(&srv, None).await;
    assert!(bob.name.starts_with("Guest-"), "{}", bob.name);
    let res = reqwest::Client::new()
        .get(srv.url("/users/me"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let me: UserInfo = User::ok(bob.get(srv.url("/users/me"))).await;
    assert_eq!(me.user_id, bob.id);
    let renamed: UserInfo = User::ok(
        bob.http
            .patch(srv.url("/users/me"))
            .bearer_auth(&bob.token)
            .json(&UpdateMeRequest {
                display_name: "Bob".into(),
            }),
    )
    .await;
    assert_eq!(renamed.display_name, "Bob");
    let bob = User {
        name: "Bob".into(),
        ..bob
    };

    // invite
    let created: CreateRoomResponse = User::ok(alice.post(srv.url("/rooms"))).await;
    assert!(proto::is_valid_invite_code(&created.invite_code));
    assert_eq!(created.snapshot.status, RoomStatus::Waiting);
    assert!(created.my_role.is_player());
    let room = created.room_id.clone();

    // Alice listens before Bob joins
    let mut alice_sse = open_sse(&srv, &alice, &room, None).await;
    let (first_id, cause, snap) = expect_state(&mut alice_sse).await;
    assert_eq!(cause, StateCause::Snapshot);
    assert_eq!(snap.status, RoomStatus::Waiting);

    // bad / unknown codes
    let (status, err) = User::err(bob.post(srv.url("/rooms/join")).json(&JoinByCodeRequest {
        invite_code: "ZZZZZZ".into(),
    }))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err.code, ErrorCode::InviteInvalid);
    let (_, err) = User::err(bob.post(srv.url("/rooms/join")).json(&JoinByCodeRequest {
        invite_code: "10".into(),
    }))
    .await;
    assert_eq!(err.code, ErrorCode::InviteInvalid);

    // sloppy code: lowercase + dash
    let sloppy = format!(
        "{}-{}",
        created.invite_code[..3].to_lowercase(),
        &created.invite_code[3..]
    );
    let joined: JoinResponse =
        User::ok(bob.post(srv.url("/rooms/join")).json(&JoinByCodeRequest {
            invite_code: sloppy,
        }))
        .await;
    assert_eq!(joined.room_id, room);
    assert_ne!(joined.my_role, created.my_role);
    assert_eq!(joined.snapshot.status, RoomStatus::Playing);
    assert!(joined.snapshot.seats.is_full());

    // Alice sees join + start
    let (_, _, ev) = next_non_presence(&mut alice_sse).await;
    match ev {
        ServerEvent::Joined { user } => assert_eq!(user.user_id, bob.id),
        other => panic!("expected joined, got {other:?}"),
    }
    let (_, cause, snap) = expect_state(&mut alice_sse).await;
    assert_eq!(cause, StateCause::Start);
    assert_eq!(snap.turn, Some(Color::Black));

    // third person: no seat
    let carol = User::register(&srv, Some("Carol")).await;
    let (status, err) = User::err(carol.post(srv.url("/rooms/join")).json(&JoinByCodeRequest {
        invite_code: created.invite_code.clone(),
    }))
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(err.code, ErrorCode::RoomFull);
    let (status, _) = User::err(carol.get(srv.url(&format!("/rooms/{room}")))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (black, white) = if created.my_role == Role::Black {
        (&alice, &bob)
    } else {
        (&bob, &alice)
    };
    let mut bob_sse = open_sse(&srv, &bob, &room, None).await;
    let (_, _, snap) = expect_state(&mut bob_sse).await;
    let mut seq = snap.seq;

    // out of turn / stale seq
    let p = Pos::from_notation("H8").unwrap();
    let (status, err) = User::err(white.post(srv.url(&format!("/rooms/{room}/move"))).json(
        &MoveRequest {
            x: p.x(),
            y: p.y(),
            expect_seq: seq,
        },
    ))
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(err.code, ErrorCode::NotYourTurn);
    let (status, err) = User::err(black.post(srv.url(&format!("/rooms/{room}/move"))).json(
        &MoveRequest {
            x: p.x(),
            y: p.y(),
            expect_seq: seq + 7,
        },
    ))
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(err.code, ErrorCode::SeqMismatch);
    assert_eq!(err.current_seq, Some(seq));

    // Black: F8 G8 / H6 H7; White on the A file
    for (i, m) in ["F8", "A1", "G8", "A2", "H6", "A3", "H7", "A4"]
        .iter()
        .enumerate()
    {
        let mover = if i % 2 == 0 { black } else { white };
        let ack = mover.play(&srv, &room, m, seq).await;
        assert_eq!(ack.seq, seq + 1);
        seq = ack.seq;
        for s in [&mut alice_sse, &mut bob_sse] {
            let (_, cause, snap) = expect_state(s).await;
            match cause {
                StateCause::Move { record } => {
                    assert_eq!(record.n as usize, i + 1);
                    assert_eq!(Pos::new(record.x, record.y).unwrap().to_string(), *m);
                }
                other => panic!("expected move cause, got {other:?}"),
            }
            assert_eq!(snap.seq, seq);
            assert_eq!(snap.moves.len(), i + 1);
        }
    }
    let snap = black.snapshot(&srv, &room).await;
    assert_eq!(snap.turn, Some(Color::Black));
    assert!(
        snap.forbidden
            .iter()
            .any(|f| f.x == p.x() && f.y == p.y() && f.kind == ForbiddenKind::DoubleThree)
    );
    assert_eq!(snap.local_forbidden().unwrap(), snap.forbidden);

    // H8 = 3-3
    let (status, err) = User::err(black.post(srv.url(&format!("/rooms/{room}/move"))).json(
        &MoveRequest {
            x: p.x(),
            y: p.y(),
            expect_seq: seq,
        },
    ))
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(err.forbidden, Some(ForbiddenKind::DoubleThree));
    // occupied
    let q = Pos::from_notation("A1").unwrap();
    let (_, err) = User::err(black.post(srv.url(&format!("/rooms/{room}/move"))).json(
        &MoveRequest {
            x: q.x(),
            y: q.y(),
            expect_seq: seq,
        },
    ))
    .await;
    assert_eq!(err.code, ErrorCode::Occupied);

    // chat
    let _: Ack = User::ok(
        alice
            .post(srv.url(&format!("/rooms/{room}/chat")))
            .json(&ChatRequest {
                text: "  hi bob  ".into(),
            }),
    )
    .await;
    for s in [&mut alice_sse, &mut bob_sse] {
        let (_, kind, ev) = next_non_presence(s).await;
        assert_eq!(kind, "chat");
        match ev {
            ServerEvent::Chat {
                from,
                display_name,
                text,
                ..
            } => {
                assert_eq!(from, alice.id);
                assert_eq!(display_name, "Alice Liddell");
                assert_eq!(text, "hi bob");
            }
            other => panic!("expected chat, got {other:?}"),
        }
    }
    let (_, err) = User::err(
        alice
            .post(srv.url(&format!("/rooms/{room}/chat")))
            .json(&ChatRequest { text: "   ".into() }),
    )
    .await;
    assert_eq!(err.code, ErrorCode::BadRequest);

    // Last-Event-ID replay (presence not persisted)
    let mut replay = open_sse(&srv, &alice, &room, Some(first_id)).await;
    let (id, kind, ev) = next_non_presence(&mut replay).await;
    assert_eq!(id, first_id + 1);
    assert_eq!(kind, "joined");
    assert!(matches!(ev, ServerEvent::Joined { .. }));
    drop(replay);
    // unknown Last-Event-ID -> snapshot
    let mut stale = open_sse(&srv, &alice, &room, Some(9_999)).await;
    let (_, cause, _) = expect_state(&mut stale).await;
    assert_eq!(cause, StateCause::Snapshot);
    drop(stale);

    // resign
    let ack: Ack = User::ok(white.post(srv.url(&format!("/rooms/{room}/resign")))).await;
    assert_eq!(ack.seq, seq + 1);
    for s in [&mut alice_sse, &mut bob_sse] {
        let (_, kind, ev) = next_non_presence(s).await;
        assert_eq!(kind, "gameover");
        match ev {
            ServerEvent::Gameover { result, snapshot } => {
                assert_eq!(result.winner, Some(Color::Black));
                assert_eq!(result.reason, EndReason::Resign);
                assert_eq!(snapshot.status, RoomStatus::Finished);
                assert_eq!(snapshot.turn, None);
                assert!(snapshot.forbidden.is_empty());
            }
            other => panic!("expected gameover, got {other:?}"),
        }
    }
    let (_, err) = User::err(black.post(srv.url(&format!("/rooms/{room}/move"))).json(
        &MoveRequest {
            x: 0,
            y: 5,
            expect_seq: seq + 1,
        },
    ))
    .await;
    assert_eq!(err.code, ErrorCode::NotPlaying);

    // next game in the same room: both Enter, loser (White) => Black
    let _: Ack = User::ok(white.post(srv.url(&format!("/rooms/{room}/ready")))).await;
    for s in [&mut alice_sse, &mut bob_sse] {
        let (_, kind, ev) = next_non_presence(s).await;
        assert_eq!(kind, "ready");
        assert!(matches!(ev, ServerEvent::Ready { .. }));
    }
    let snap = black.snapshot(&srv, &room).await;
    assert_eq!(snap.ready, vec![white.id.clone()]);
    assert_eq!(snap.games_played, 1);
    let ack: Ack = User::ok(black.post(srv.url(&format!("/rooms/{room}/ready")))).await;
    for s in [&mut alice_sse, &mut bob_sse] {
        let (_, kind, _) = next_non_presence(s).await;
        assert_eq!(kind, "ready");
        let (_, cause, snap) = expect_state(s).await;
        assert_eq!(cause, StateCause::Start);
        assert_eq!(snap.status, RoomStatus::Playing);
        assert_eq!(snap.seq, ack.seq);
        assert!(snap.moves.is_empty());
        assert!(snap.ready.is_empty());
        assert_eq!(snap.result, None);
        assert_eq!(snap.seats.black.as_ref().unwrap().user_id, white.id);
        assert_eq!(snap.seats.white.as_ref().unwrap().user_id, black.id);
    }
    let (_, err) = User::err(black.post(srv.url(&format!("/rooms/{room}/ready")))).await;
    assert_eq!(err.code, ErrorCode::NotPlaying);

    // second game to a five
    let (black2, white2) = (white, black);
    let mut seq2 = ack.seq;
    for (i, m) in ["H8", "A1", "I8", "A2", "J8", "A3", "K8", "A4"]
        .iter()
        .enumerate()
    {
        let mover = if i % 2 == 0 { black2 } else { white2 };
        seq2 = mover.play(&srv, &room, m, seq2).await.seq;
        expect_state(&mut alice_sse).await;
        expect_state(&mut bob_sse).await;
    }
    black2.play(&srv, &room, "L8", seq2).await;
    for s in [&mut alice_sse, &mut bob_sse] {
        let (_, cause, snap) = expect_state(s).await;
        assert!(matches!(cause, StateCause::Move { .. }));
        assert_eq!(snap.status, RoomStatus::Finished);
        let (_, kind, ev) = next_non_presence(s).await;
        assert_eq!(kind, "gameover");
        match ev {
            ServerEvent::Gameover { result, snapshot } => {
                assert_eq!(result.winner, Some(Color::Black));
                assert_eq!(result.reason, EndReason::Five);
                assert_eq!(result.line.len(), 5);
                assert_eq!(snapshot.games_played, 2);
            }
            other => panic!("expected gameover, got {other:?}"),
        }
    }

    // "restart": fresh registry over the same database
    let restarted = AppState::new(srv.pool.clone(), Config::default());
    let rebuilt = restarted.room(&room).await.unwrap().snapshot().await;
    assert_eq!(rebuilt.status, RoomStatus::Finished);
    assert_eq!(rebuilt.moves.len(), 9);
    assert_eq!(rebuilt.games_played, 2);
    assert_eq!(rebuilt.result.as_ref().unwrap().winner, Some(Color::Black));
    assert_eq!(rebuilt.result.as_ref().unwrap().line.len(), 5);
    assert_eq!(rebuilt.seats.black.as_ref().unwrap().user_id, white.id);
    srv.cleanup().await;
}

#[tokio::test]
async fn presence_and_offline_are_reported() {
    let srv = TestServer::start().await;
    let alice = User::register(&srv, Some("Alice")).await;
    let bob = User::register(&srv, Some("Bob")).await;
    let created: CreateRoomResponse = User::ok(alice.post(srv.url("/rooms"))).await;
    let room = created.room_id.clone();
    let _: JoinResponse = User::ok(bob.post(srv.url("/rooms/join")).json(&JoinByCodeRequest {
        invite_code: created.invite_code.clone(),
    }))
    .await;

    let mut alice_sse = open_sse(&srv, &alice, &room, None).await;
    expect_state(&mut alice_sse).await;
    let bob_sse = open_sse(&srv, &bob, &room, None).await;
    let (_, kind, ev) = next(&mut alice_sse).await;
    assert_eq!(kind, "presence");
    assert_eq!(
        ev,
        ServerEvent::Presence {
            user_id: bob.id.clone(),
            display_name: "Bob".into(),
            online: true
        }
    );
    let snap = alice.snapshot(&srv, &room).await;
    assert!(snap.seats.by_user(&bob.id).unwrap().online);

    drop(bob_sse);
    let (_, kind, ev) = next(&mut alice_sse).await;
    assert_eq!(kind, "presence");
    assert_eq!(
        ev,
        ServerEvent::Presence {
            user_id: bob.id.clone(),
            display_name: "Bob".into(),
            online: false
        }
    );
    let snap = alice.snapshot(&srv, &room).await;
    assert!(!snap.seats.by_user(&bob.id).unwrap().online);
    srv.cleanup().await;
}

#[tokio::test]
async fn expired_invite_is_rejected_but_members_can_return() {
    let srv = TestServer::start().await;
    let alice = User::register(&srv, Some("Alice")).await;
    let bob = User::register(&srv, Some("Bob")).await;
    let created: CreateRoomResponse = User::ok(alice.post(srv.url("/rooms"))).await;

    // age the invite in the database
    sqlx::query("UPDATE rooms SET invite_expires_at = '2000-01-01T00:00:00Z' WHERE id = $1")
        .bind(uuid::Uuid::parse_str(&created.room_id).unwrap())
        .execute(&srv.pool)
        .await
        .unwrap();
    // fresh registry => expiry reloaded
    let restarted = AppState::new(srv.pool.clone(), Config::default());
    let app = build_app(restarted);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let base = format!("http://{addr}");

    let (status, err) = User::err(bob.post(format!("{base}/rooms/join")).json(
        &JoinByCodeRequest {
            invite_code: created.invite_code.clone(),
        },
    ))
    .await;
    assert_eq!(status, StatusCode::GONE);
    assert_eq!(err.code, ErrorCode::InviteExpired);

    // creator: still allowed back in
    let back: JoinResponse = User::ok(alice.post(format!("{base}/rooms/join")).json(
        &JoinByCodeRequest {
            invite_code: created.invite_code,
        },
    ))
    .await;
    assert_eq!(back.my_role, created.my_role);
    srv.cleanup().await;
}
