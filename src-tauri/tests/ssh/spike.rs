//! 세션 정지(R1) 스파이크: **같은 연결에서 채널 A 의 출력을 읽지 않는 동안 채널 B 의 실행이 끝나는가.**
//! russh 는 받은 데이터를 채널마다 유한한 큐로 넘기고(`chan.send(..).await`) 윈도우도 받은 바이트로 조정하므로,
//! 읽지 않는 채널이 세션 루프 전체를 멈출 수 있다는 것이 소스 읽기로 예상한 바다. 이 시험이 그것을 실측하고 펌프(읽는 쪽이 항상 비운다)로 풀리는지 본다.
//! 날 russh 클라이언트를 쓰는 유일한 시험이라 연결·인증 함수를 직접 부른다(제품 코드는 connection.rs 만 부른다).
#![allow(clippy::disallowed_methods)]

use std::sync::Arc;
use std::time::Duration;

use russh::keys::PrivateKeyWithHashAlg;
use russh::{client, Channel, ChannelMsg};

use super::server::{new_key, Opts, TestServer};

struct AcceptAll;

impl client::Handler for AcceptAll {
    type Error = russh::Error;
    async fn check_server_key(&mut self, _: &russh::keys::PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

async fn connect(server: &TestServer, key: russh::keys::PrivateKey) -> client::Handle<AcceptAll> {
    let mut h = client::connect(Arc::new(client::Config::default()), ("127.0.0.1", server.port), AcceptAll).await.unwrap();
    assert!(h.authenticate_publickey("t", PrivateKeyWithHashAlg::new(Arc::new(key), None)).await.unwrap().success());
    h
}

async fn exec(h: &client::Handle<AcceptAll>, cmd: &str) -> Channel<client::Msg> {
    let ch = h.channel_open_session().await.unwrap();
    ch.exec(true, cmd).await.unwrap();
    ch
}

/// 채널 하나를 닫힐 때까지 읽어 받은 바이트 수를 센다
async fn read_to_end(ch: &mut Channel<client::Msg>) -> usize {
    let mut n = 0;
    while let Some(m) = ch.wait().await {
        if let ChannelMsg::Data { data } = m {
            n += data.len();
        }
    }
    n
}

/// 4MB 를 1KB 패킷으로 쏟는다 — russh 의 채널 큐는 **메시지 100개**라 윈도우(2MB)보다 먼저 찬다
const FLOOD: &str = "'antler-fake' 'flood' '4194304' '1024'";

/// 읽지 않는 채널 A(4MB 를 쏟는다)가 있을 때 채널 B(작은 출력)가 끝나는지: (읽지 않을 때, 펌프가 비울 때)
pub async fn measure() -> (bool, bool) {
    let key = new_key();
    let server = TestServer::start(Opts::new(vec![key.public_key().clone()])).await;
    let limit = Duration::from_secs(4);

    // 1) A 를 읽지 않는다. A 의 흐름이 멈춘(서버가 더 못 보내는) 것을 확인한 **뒤에** B 를 연다 — 그 전에는 B 가 A 보다 먼저 끝나 버려 시험이 되지 않는다
    let h = connect(&server, key.clone()).await;
    let _a = exec(&h, FLOOD).await;
    // 서버의 전송이 연속 5번 같고 전체보다 작을 때 멈춘 것으로 본다(굼뜬 러너에서 한 번 같은 것만으로 판정하지 않는다)
    let (mut last, mut same) = (0, 0);
    for _ in 0..300 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let now = server.flooded();
        same = if now > 0 && now == last { same + 1 } else { 0 };
        last = now;
        if same >= 5 {
            break;
        }
    }
    assert!(same >= 5 && last < 4 * 1024 * 1024, "A 의 전송이 중간에 멈추지 않았다({last}바이트) — 시험 전제가 깨졌다");
    println!("[spike] 읽지 않는 A 로 서버가 보낸 바이트 {last} (전체 4194304 중), 서버 자식 {}", server.active_children());
    let b_started = tokio::time::timeout(limit, async {
        let mut b = exec(&h, "'antler-fake' 'flood' '10'").await;
        read_to_end(&mut b).await
    })
    .await;
    println!("[spike] unread: B {b_started:?}");
    let unread_ok = b_started.is_ok();
    drop(h);

    // 2) A 를 펌프가 즉시 비운다
    let h = connect(&server, key).await;
    let mut a = exec(&h, FLOOD).await;
    let pump = tokio::spawn(async move { read_to_end(&mut a).await });
    let mut b = exec(&h, "'antler-fake' 'flood' '10'").await;
    let pumped_ok = tokio::time::timeout(limit, read_to_end(&mut b)).await.is_ok();
    let a_total = tokio::time::timeout(Duration::from_secs(30), pump).await.ok().and_then(Result::ok).unwrap_or(0);
    assert!(!pumped_ok || a_total == 4 * 1024 * 1024, "펌프가 A 의 4MB 를 다 받아야 한다: {a_total}");
    (unread_ok, pumped_ok)
}
