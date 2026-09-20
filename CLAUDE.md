# gomoku

터미널에서 친구와 두는 온라인 오목(렌주 룰). Rust 워크스페이스 4개 크레이트. 설계 문서는 따로 없고 이 파일이 전부다.

## 설계 요약

- 턴제·완전정보·초대 기반. 실시간성·매치메이킹·로비 없음. 로그인 없음: 익명 토큰이 곧 계정(`users` + `credentials`), 나중에 `identities`로 신원만 연결
- 통신은 HTTP POST + SSE(WebSocket 아님). 재접속은 `Last-Event-ID` 재생, keepalive 15초, 프록시 버퍼링 방지 헤더(`X-Accel-Buffering: no`)
- 서버는 파드 1개. 방 상태(`RoomHub`)와 팬아웃(`tokio::broadcast`)이 메모리라 다중 파드 불가. 필요해지면 상태를 DB 트랜잭션으로, 팬아웃을 Postgres `LISTEN/NOTIFY`로 옮기는 것이 확장 경로
- 선공은 서버가 무작위. 대국 종료 후 양쪽 Enter(`/ready`)로 같은 방 재시작, 패자가 흑, 무승부는 색 교대
- 확장 자리만 있는 것: `Role::Spectator`, `Visibility::Public`, `identities`, `EndReason::Timeout`
- TUI 레이아웃: 폭 64+ 좌우 분할, 34~63 상하, 20~33 컴팩트 보드, 미만 안내. 높이 22 미만이면 분할 강제. 보드 폭 32/18, 고정 크기 가운데 정렬

## 구조

| 크레이트 | 역할 | 규칙 |
|---|---|---|
| `rules/` | 렌주 규칙 엔진 (`Board`, `Pos`, `Game`, `forbidden_reason`, `check_win`) | **외부 의존성 금지.** 테스트는 `((돌 배치, 좌표), 기대값)` 케이스로 쌓는다 |
| `proto/` | 요청/응답/SSE 이벤트 타입 (serde). `rules`에만 의존 | 서버·클라이언트가 공유하므로 여기서 필드를 바꾸면 양쪽이 같이 컴파일 에러로 잡힌다 |
| `server/` | axum 0.8 + sqlx 0.9(PostgreSQL). 바이너리 `gomoku-server` | 상태 변경은 `room.rs`의 `RoomHub` 메서드 안(락 보유)에서만. 이벤트는 `emit()`으로 DB 기록 후 브로드캐스트. 쿼리는 `db.rs` 한 파일에 |
| `client/` | clap + ratatui 0.30 + reqwest. 바이너리 `gomoku` | 네트워크와 무관한 상태 로직은 `session.rs`, 네트워크 글루는 `driver.rs`, UI는 `tui/`·`plain.rs` |

## 자주 쓰는 명령

```bash
scripts/dev-db.sh up                       # docker compose로 PostgreSQL 17 기동 (reset: 볼륨 삭제 후 재생성)
cp .env.example .env                       # 서버·테스트가 읽는 접속 정보

cargo build --workspace
cargo test --workspace                    # server 통합 테스트는 Postgres 필요 (GOMOKU_TEST_DATABASE_URL, 테스트별 DB 생성/삭제)
cargo clippy --workspace --all-targets    # 경고 0개 유지 (pedantic 활성)
cargo fmt --all --check

cargo srv                                  # = cargo run -p server --   (127.0.0.1:3000, GOMOKU_DATABASE_URL)
cargo cli invite                           # = cargo run -p gomoku --bin gomoku -- invite
cargo cli --plain join ABC234              # 라인 모드 (파이프/디버깅용)
GOMOKU_CONFIG_DIR=/tmp/p2 cargo cli join ABC234      # 같은 머신에서 두 번째 플레이어 흉내
```

서버 디버깅: `curl -N -H "Authorization: Bearer $TOKEN" localhost:3000/rooms/$ROOM/events`.

## 불변 규칙

- 착수 확정은 서버 이벤트로만. 클라이언트는 낙관적 업데이트를 하지 않는다. 로컬 규칙 검증은 **거절할 때만** 신뢰한다.
- 모든 `state`/`gameover` 이벤트는 전체 `RoomSnapshot`을 싣는다. 부분 업데이트를 추가하지 않는다.
- 두 시퀀스를 혼동하지 말 것: `snapshot.seq`(상태 버전, `expect_seq`용)와 SSE `id`(이벤트 로그 번호, `Last-Event-ID`용).
- 보드 상태는 저장하지 않는다. `moves` 테이블이 현재 판의 착수 목록이고 보드는 재구성한다. 다음 판이 시작되면 비워진다(`db::restart_game`). 스키마 변경은 `server/migrations/`에 새 파일로 추가한다. 이미 적용된 마이그레이션 파일은 주석 한 글자도 고치지 않는다(체크섬 불일치로 기동 실패; 로컬은 `scripts/dev-db.sh reset`).
- 금수 판정 순서: 5목 → 장목 → 4-4 → 3-3. 5목이 금수보다 우선한다. 3의 판정은 재귀적이며 `MAX_DEPTH`로 제한한다.
- 보드 문자는 ASCII(`X`, `O`, `.`, `#`)만. `●` 같은 East Asian Ambiguous 폭 문자는 쓰지 않는다.
- 크로스 플랫폼: Unix 전용 API는 `#[cfg(unix)]` 안에서만. 키 입력은 `KeyEventKind::Press`만 처리한다(Windows는 Release도 보낸다).
- 게임 안 명령은 좌표·채팅·`/resign`·`/help`·`/quit`만. 무르기, 재대국 협상, 기보/코드/다시그리기 명령은 사용자가 뺀 것이므로 다시 넣지 않는다. 대국 종료 후 재시작은 양쪽 `POST /rooms/{id}/ready`.
- 관전(`Role::Spectator`), `Visibility::Public`, `Identity` 테이블은 모델에만 있고 UI에 노출하지 않는다. 기능을 붙이기 전까지 그대로 둔다.

## 주석 규칙

- 코드로 설명되지 않는 **의사결정**이 있을 때만, 제한적으로. 코드가 하는 일을 다시 말하는 주석 금지
- 개조식 한 줄. `~한다`/`~된다` 같은 서술형 종결 금지, 온점 지양, 연결은 `;` `=` `=>` `,`
- `/// Returns X` 류 doc 삭제 대상. `//!` 모듈 문서는 2~3줄 이내, 불변 규칙이나 함정만
- Rust 뿐 아니라 YAML·TOML·SQL·Dockerfile·워크플로 주석에도 동일 적용

## 의존성 메모 (2026-09 기준 최신 API)

- sqlx 0.9 + PostgreSQL: `query()`는 `&'static str`만 받는다(`$1` 플레이스홀더). 동적 SQL은 테스트의 `CREATE DATABASE`처럼 `AssertSqlSafe`로만. 순수 Rust 드라이버라 C 빌드 없음. id는 DB에서 UUID, 코드에서는 String(`db.rs` 경계에서 변환).
- rand 0.10: `rand::rng()`, `RngExt::random_range`, `Rng::fill_bytes`.
- ratatui 0.30: `ratatui::init()/restore()`, crossterm은 `ratatui::crossterm`으로 재수출(0.29).
- reqwest 0.13 / sqlx TLS: rustls(aws-lc-rs). C 컴파일러가 빌드에 필요하다.
- edition 2024, MSRV 1.94 (sqlx 요구).

## 버전

- 워크스페이스 단일 버전(`[workspace.package].version`). 네 크레이트가 같은 번호. **버전을 손으로 고치지 않는다.** release-plz가 커밋 메시지로 올린다.
- SemVer. 1.0 전: 깨지는 변경 = MINOR, 나머지 = PATCH. 프로토콜(`proto`)이 호환되지 않게 바뀌면 `proto::PROTOCOL_VERSION`도 +1.
- 커밋 메시지는 Conventional Commits. 이것이 버전을 결정한다.
  - `feat:` → MINOR, `fix:` `perf:` `refactor:` → PATCH, `feat!:` 또는 본문 `BREAKING CHANGE:` → 깨지는 변경
  - `chore:` `docs:` `test:` `ci:` → 버전에 영향 없음, CHANGELOG 제외
  - **커밋 메시지는 한국어 한 줄로 간결하게.** 본문 없음, 영문 요약 금지. type과 scope만 영문 (예: `feat(client): 커서 이동으로 착수`, `fix(server): 종료 시 SSE 스트림 정리`)
  - 깨지는 변경만 예외적으로 `feat!(proto): ...` 뒤에 `BREAKING CHANGE:` 본문 한 줄 허용
- 태그는 `vX.Y.Z` 하나(release-plz가 client 패키지에만 붙임). GitHub Release는 dist가 태그를 받아 만든다.

## 릴리스·배포

- 서버: `main` 푸시마다 `server-image.yml`이 GHCR `ghcr.io/seol-jy/gomoku-server:<sha>`를 올리고 `../seol-gitops/apps/gomoku/kustomization.yaml`의 `newTag`를 갱신 → Argo CD 배포. 서버가 항상 클라이언트보다 먼저 나가므로 **서버는 직전 클라이언트와 호환되어야 한다.**
- 클라이언트: `release-plz.yml`이 릴리스 PR을 유지하고, 머지되면 태그 → `release.yml`(dist 생성, 손대지 않음)이 바이너리·셸 설치기·Homebrew formula(`Seol-JY/homebrew-tap`)·attestation을 올린다.
- 클라이언트 릴리스 뒤 후속 조치: `seol-gitops/apps/gomoku/deployment.yaml`의 `GOMOKU_LATEST_CLIENT_VERSION`을 새 버전으로 올린다(소프트 안내).
- 깨지는 변경 순서: 서버 호환 유지 → 서버 배포 → 클라이언트 릴리스 + LATEST 상향 → 유예 2주 → `GOMOKU_MIN_CLIENT_VERSION` 상향(426 강제) → 다음 릴리스에서 호환 분기 제거.
- 배포 형태: 파드 1개 + `Recreate`(방 상태가 메모리에 있어 다중 파드 불가). `/healthz`는 DB를 보지 않고 `/readyz`만 본다. 종료는 SSE를 끊고 `GOMOKU_SHUTDOWN_TIMEOUT_SECS` 대기.
- 릴리스 빌드의 기본 서버 주소는 `client/build.rs`가 굽는다(release → `https://api-gomoku.seol.pro`, debug → localhost, `GOMOKU_DEFAULT_SERVER`로 덮어쓰기).
- 되돌리기: 서버는 `newTag`를 이전 SHA로. 마이그레이션은 추가만 하고 컬럼 삭제는 하지 않아 구버전이 새 스키마에서도 돌게 한다.
- PAT 없을 때의 동작: `server-image.yml`은 이미지만 올리고 gitops 태그 갱신은 건너뜀(`newTag` 수동), `release-plz.yml`은 GITHUB_TOKEN으로 PR만 만들고 태그는 `release.yml`을 깨우지 못함(태그 수동 푸시). PAT(`GITOPS_TOKEN`, `RELEASE_PLZ_TOKEN`, `HOMEBREW_TAP_TOKEN`) 등록 시 전부 자동화
- 커밋 주체는 항상 `Seol-JY <wlsdud5654@gmail.com>`(레포 로컬 git config). 전역 설정은 회사 계정이라 새 클론마다 다시 지정
- 서버 보안: 익명 등록에 rate limit 없음(친구용이라 보류), 토큰 만료 없음(`credentials.expires_at` NULL). 필요해지면 컬럼은 이미 있음

## 하지 않는 것

- 커밋/푸시는 사용자가 명시적으로 요청할 때만. 요청 시 메시지는 위 "버전" 절 형식(한국어 한 줄)으로.
