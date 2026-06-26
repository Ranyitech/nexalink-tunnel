use iroh::{
    Endpoint, EndpointAddr, RelayMap, RelayMode, RelayUrl, SecretKey,
    endpoint::presets,
};
use n0_error::{Result, StdResultExt};
use std::env;
use std::path::Path;
use tokio::io::AsyncWriteExt; // 引入写操作所需的 trait
use tokio::time::{Duration, Instant};

const ALPN: &[u8] = b"ranyitech.com/perf-test/v1";

async fn load_or_create_key(path: impl AsRef<Path>) -> Result<SecretKey> {
    let path = path.as_ref();
    if path.exists() {
        let bytes = tokio::fs::read(path).await.anyerr()?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "密钥损坏")
        }).anyerr()?;
        Ok(SecretKey::from_bytes(&bytes))
    } else {
        let key = SecretKey::generate();
        tokio::fs::write(path, key.to_bytes()).await.anyerr()?;
        Ok(key)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        println!("用法: cargo run --bin perf_client '<SERVER_JSON_ADDRESS>'");
        std::process::exit(0);
    }
    let addr_json = &args[1];
    let addr: EndpointAddr = serde_json::from_str(addr_json).anyerr()?;

    let secret_key = load_or_create_key("client_perf.key").await?;
    let relay_cn: RelayUrl = "https://relay-gz.ranyitech.com".parse().anyerr()?;
    let relay_us: RelayUrl = "https://relay-n.nexalink.link".parse().anyerr()?;
    let relay_map = RelayMap::from_iter([relay_cn, relay_us]);

    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret_key)
        .relay_mode(RelayMode::Custom(relay_map))
        .bind()
        .await?;

    println!("正在连接服务端建立安全通道...");
    let conn = endpoint.connect(addr, ALPN).await.anyerr()?;
    println!("物理通道建立成功！开启性能测试...");

    // ==========================================
    // 1. 延迟和网络抖动测试 (Ping-Pong)
    // ==========================================
    println!("\n[1/2] 正在进行延迟与抖动测试 (50次采样)...");
    let (mut send, mut recv) = conn.open_bi().await.anyerr()?;
    send.write_all(&[1]).await.anyerr()?; // 写入指令 1 (延迟测试)

    let mut rtts = Vec::new();
    for _ in 0..50 {
        let start = Instant::now();
        let payload = [0u8; 8];
        send.write_all(&payload).await.anyerr()?;
        let mut resp = [0u8; 8];
        recv.read_exact(&mut resp).await.anyerr()?;
        let rtt = start.elapsed().as_secs_f64() * 1000.0; // 毫秒
        rtts.push(rtt);
        tokio::time::sleep(Duration::from_millis(30)).await; // 每次间隔 30ms 采样
    }

    // 统计结果
    let min_rtt = rtts.iter().cloned().fold(f64::INFINITY, f64::min);
    let max_rtt = rtts.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let avg_rtt = rtts.iter().sum::<f64>() / rtts.len() as f64;
    // Jitter 计算：相邻采样延迟差值的绝对值的平均值
    let jitter = if rtts.len() > 1 {
        let diff_sum: f64 = rtts.windows(2).map(|w| (w[1] - w[0]).abs()).sum();
        diff_sum / (rtts.len() - 1) as f64
    } else {
        0.0
    };

    println!("--------------------------------------------------");
    println!("延迟探测完成！统计结果如下：");
    println!("  最小延迟 (Min RTT): {:.2} ms", min_rtt);
    println!("  最大延迟 (Max RTT): {:.2} ms", max_rtt);
    println!("  平均延迟 (Avg RTT): {:.2} ms", avg_rtt);
    println!("  网络抖动 (Jitter) : {:.2} ms (平均相邻延迟抖动)", jitter);
    println!("--------------------------------------------------");

    // ==========================================
    // 2. 带宽与吞吐量测试 (拉取数据)
    // ==========================================
    println!("\n[2/2] 正在进行带宽吞吐量测试 (拉取 200MB 数据)...");
    let (mut send, mut recv) = conn.open_bi().await.anyerr()?;
    send.write_all(&[2]).await.anyerr()?; // 写入指令 2 (带宽测试)

    let start = Instant::now();
    let mut total_read = 0;
    let mut buf = vec![0u8; 64 * 1024]; // 64KB 读取缓冲
    loop {
        // 【核心修复】：适配 Iroh 1.0.0 的 Option<usize> 返回值
        match recv.read(&mut buf).await {
            Ok(None) => break,           // EOF，数据接收完毕
            Ok(Some(n)) => total_read += n, // 成功读取了 n 个字节
            Err(e) => {
                eprintln!("接收吞吐数据异常中断: {:?}", e);
                break;
            }
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    let total_mb = total_read as f64 / (1024.0 * 1024.0);
    // Mbps 换算： (字节数 * 8位) / (耗时(秒) * 1,000,000)
    let mbit_per_sec = (total_read as f64 * 8.0) / (elapsed * 1_000_000.0);

    println!("--------------------------------------------------");
    println!("吞吐带宽测试完成！统计结果如下：");
    println!("  传输耗时 (Duration) : {:.3} 秒", elapsed);
    println!("  数据接收 (Data Size): {:.2} MB", total_mb);
    println!("  实际带宽 (Bandwidth): {:.2} Mbps", mbit_per_sec);
    println!("--------------------------------------------------");


    // ==================【核心修改：优雅关闭连接】==================
    println!("\n正在优雅关闭本地测试端点...");
    endpoint.close().await;
    println!("测试运行全部完成，已安全退出。");

    Ok(())
}
