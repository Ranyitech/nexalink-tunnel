use iroh::{
    Endpoint, RelayMap, RelayMode, RelayUrl, SecretKey,
    endpoint::{Connection, presets, QuicTransportConfig},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use n0_error::{Result, StdResultExt};
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
    
    let secret_key = load_or_create_key("server_perf.key").await?;
    let relay_cn: RelayUrl = "https://relay-gz.ranyitech.com".parse().anyerr()?;
    let relay_us: RelayUrl = "https://relay-n.nexalink.link".parse().anyerr()?;
    let relay_map = RelayMap::from_iter([relay_cn, relay_us]);

    // 生产对等：保活心跳与空闲超时
    let transport_config = QuicTransportConfig::builder()
        .keep_alive_interval(std::time::Duration::from_secs(15))
        .max_idle_timeout(std::time::Duration::from_secs(60).try_into().ok())
        .build();
    
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret_key)
        .relay_mode(RelayMode::Custom(relay_map))
        .transport_config(transport_config)
        .bind()
        .await?;

    let router = Router::builder(endpoint)
        .accept(ALPN, PerfHandler)
        .spawn();
        
    router.endpoint().online().await;
    
    let addr = router.endpoint().addr();
    let addr_json = serde_json::to_string(&addr).anyerr()?;
    
    println!("\n==================================================");
    println!("性能测试服务端已启动（100% 生产同构无 FEC）！正在守候连接...");
    println!("请将以下地址复制并传递给测试客户端：");
    println!("{}", addr_json);
    println!("==================================================\n");
    
    tokio::signal::ctrl_c().await.anyerr()?;
    router.shutdown().await.anyerr()?;
    Ok(())
}

#[derive(Debug, Clone)]
struct PerfHandler;

impl ProtocolHandler for PerfHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        println!("【测试通道已建立】外部客户端已接入: {}", connection.remote_id());
        
        loop {
            let (mut send, mut recv) = match connection.accept_bi().await {
                Ok(streams) => streams,
                Err(_) => break,
            };

            tokio::spawn(async move {
                let mut cmd = [0u8; 1];
                if recv.read_exact(&mut cmd).await.is_err() {
                    return;
                }

                match cmd[0] {
                    1 => {
                        // 阶段一：Ping-Pong 压缩测试
                        for _ in 0..50 {
                            match read_decompressed(&mut recv).await {
                                Ok(Some(payload)) => {
                                    if write_compressed(&mut send, &payload).await.is_err() {
                                        break;
                                    }
                                }
                                _ => break,
                            }
                        }
                    }
                    2 => {
                        // 阶段二：无阻断纯净带宽测试。下发 100MB 混合 Web 仿真流量（预估压缩比 75%）
                        let mut seed = 123456789u32;
                        let chunk_size = 16 * 1024; // 16KB
                        let total_bytes = 200 * 1024 * 1024; // 100 MB
                        let target_ratio = 0.75;
                        
                        let mut sent = 0;
                        while sent < total_bytes {
                            let to_send = std::cmp::min(chunk_size, total_bytes - sent);
                            let raw_chunk = generate_semi_compressible_chunk(to_send, target_ratio, &mut seed);
                            
                            // 服务端直接写压缩流（无任何 FEC 冗余和阻断延迟）
                            if write_compressed(&mut send, &raw_chunk).await.is_err() {
                                break;
                            }
                            sent += to_send;
                        }
                        let _ = send.finish();
                    }
                    _ => {}
                }
            });
        }
        Ok(())
    }
}

async fn write_compressed<W>(writer: &mut W, data: &[u8]) -> std::io::Result<()>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let compressed = lz4_flex::compress(data);
    let len = compressed.len() as u32;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&compressed).await?;
    Ok(())
}

async fn read_decompressed<R>(reader: &mut R) -> std::io::Result<Option<Vec<u8>>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    
    let mut compressed = vec![0u8; len];
    reader.read_exact(&mut compressed).await?;
    
    let decompressed = lz4_flex::decompress(&compressed, 128 * 1024)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(Some(decompressed))
}

fn generate_semi_compressible_chunk(size: usize, target_ratio: f64, seed: &mut u32) -> Vec<u8> {
    let mut buf = vec![0u8; size];
    let random_bytes_count = (size as f64 * target_ratio).round() as usize;
    let random_bytes_count = std::cmp::min(random_bytes_count, size);
    for i in 0..random_bytes_count {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        buf[i] = (*seed >> 24) as u8;
    }
    for i in random_bytes_count..size {
        buf[i] = 0;
    }
    buf
}
