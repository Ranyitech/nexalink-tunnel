docker run -d \
  --name iroh-relay \
  --restart unless-stopped \
  -v /root/nexalink-tunnel:/nexalink-tunnel \
  -e RUST_LOG=info \
  -p 3443:443 \
  -p 7830:7830/udp \
  n0computer/iroh-relay \
  --config-path /nexalink-tunnel/config.toml
