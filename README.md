# gomoku

터미널에서 두는 온라인 오목

```bash
brew install Seol-JY/tap/gomoku   # 또는: curl --proto '=https' --tlsv1.2 -LsSf https://github.com/Seol-JY/gomoku/releases/latest/download/gomoku-installer.sh | sh
gomoku invite
gomoku join ABC234
gomoku rejoin
```

| 입력 | 동작 |
|---|---|
| `h8` | 착수 |
| 화살표 → Enter | 입력줄이 비어 있을 때 커서를 옮겨 착수 |
| Enter (판이 끝난 뒤) | 다음 판 |
| `/resign` `/help` `/quit` | 기권, 도움말, 나가기(대국은 유지) |

흑의 금수 자리는 `#`로 표시된다.

## 개발

```
rules/    렌주 규칙 (의존성 없음)      proto/    요청/응답/이벤트 타입
server/   axum + PostgreSQL           client/   clap + ratatui, 바이너리 gomoku
```

```bash
scripts/dev-db.sh up && cp .env.example .env
cargo run -p server
cargo run -p gomoku --bin gomoku -- invite
GOMOKU_CONFIG_DIR=/tmp/p2 cargo run -p gomoku --bin gomoku -- join <CODE>
cargo test --workspace && cargo clippy --workspace --all-targets
```

요구 사항: Rust 1.94+, C 컴파일러, PostgreSQL 15+(docker compose 제공). macOS·Linux·Windows.

## 라이선스

MIT. [`LICENSE`](LICENSE) 참고.
