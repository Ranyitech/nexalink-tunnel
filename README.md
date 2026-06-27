Nexalink Tunnel
Nexalink Tunnel 是由深圳市然壹科技有限公司开发并开源的、基于现代网络协议的去中心化安全网络隧道与穿透引擎。

项目旨在解决复杂广域网和严格防火墙环境下的设备互联难题，提供高稳定性、低延迟且安全的边缘网络传输通道。非常适合用于内网 AI 服务暴露（如 Ollama、LocalAI）、远程桌面连接、私有 API 网关代理以及跨地域边缘计算节点互联。

#核心特性
原生 QUIC 传输： 基于 UDP 协议的多路复用（Multiplexing）设计，彻底摆脱传统的 TCP 队头阻塞。在高延迟、高丢包的跨境或弱网环境下依然能提供平滑的数据传输。
自适应 WebSocket 回退（WSS）： 当检测到物理网络严格封锁了 UDP 通信时，系统会无缝回退到基于标准的 TCP 443（HTTPS/WebSocket）加密中转通路，保障隧道 100% 的连通性。
LZ4 极速流式分帧压缩： 集成纯 Rust 实现的 lz4_flex 编解码引擎。对数据载荷采用自描述 4 字节长度前缀进行流式分帧，无感压缩。在传输 JSON 报文、API 文本或数据库日志时，能有效节省约 25%~70% 的网口物理带宽。
零信任白名单审计： 基于 Noise 协议框架与 Curve25519 密钥交换体系，实现强身份节点验证。非授权客户端在完成底层物理握手前即被连接拦截，确保源站安全。
15s 物理活性探测与自愈： 配置了 15 秒主动双向 Keep-Alive 心跳包，能有效锁定运营商 Carrier-Grade NAT（CGNAT）及家用网关的 UDP 端口映射。辅以 60 秒空闲自检，防止连接静默死锁。
物理连接复用与连接漫游： 多个逻辑数据流（Bi-directional Streams）共享同一个高强度加密物理连接，消除重复握手的时延。支持物理网络切换时的连接自动漫游。


#安装与编译
Nexalink 采用纯 Rust 开发，无任何 C 语言库依赖，可非常方便地交叉编译到各类主流平台。

编译 Release 版本
在生产环境部署时，建议使用 --release 参数进行编译，以获取最极致的 LZ4 编解码速度与传输能效：

编译服务端与客户端
cargo build --release --bin tunnel-listener(Server)
cargo build --release --bin tunnel-sender(Client)

部署与使用指南
所有的核心配置均已重构为动态参数解析驱动，生产环境中无需重复编译即可轻松通过命令行调整运行配置。

1. 服务端部署（Server）
在需要被暴露的内网服务器上运行服务端。

指定参数启动：
例如：需要穿透位于内网 127.0.0.1:11434 的本地 AI 模型服务，限制只允许特定 EndpointID（白名单）连接，并设置自定义的隧道协议名称：

./tunnel-listen -l "127.0.0.1:11434" -w "c2381bc8c69ce7ca39804b9b3edb04c014170500866272867ccc0a7daae505ad" -a "ranyitech.com/port-forwarding/v1"

启动成功后，服务端会在控制台打印出一段永久有效的 JSON 格式地址，请将其安全地复制到客户端使用。

2.客户端部署（Client）
在需要调用服务的外部客户端电脑上运行客户端。

自定义监听端口与 ALPN 连接：
例如：连接服务端，并指定在外网本地绑定 25444 端口对外暴露：

./tunnel-send '<SERVER_JSON_ADDRESS>' -p "0.0.0.0:25444" -a "ranyitech.com/port-forwarding/v1"

启动成功后，你可以直接通过本地访问 http://localhost:25444 安全、稳定地调用远在内网的 AI 服务。

3.查看帮助
你可以通过 --help 或 -h 选项随时获取最新的命令行参数指南：

./server --help
./client --help

#商业授权与技术支持
Nexalink 遵循开源协议，由深圳市然壹科技有限公司持续维护。我们致力于为企业提供低时延、抗丢包、高度可定制的边缘网络组网方案。

企业合作与商业授权咨询： support@ranyitech.com
官方网站： https://www.nexalink.link

