# rust-network-mgr — Claude Code Instructions

## Project Overview

Linux network management daemon written in Rust. Monitors interfaces via rtnetlink, manages nftables firewall rules, tracks Docker containers, and exposes an HTTP REST API + MCP server for AI tool integration.

## Build & Run

```bash
cargo build --release

# Start daemon (requires root for nftables/netlink)
sudo ./target/release/rust-network-mgr

# Run MCP server (connects to HTTP API)
./target/release/rust-network-mgr-mcp --api-url http://127.0.0.1:9100
```

## Development

```bash
cargo check          # Fast compile check
cargo clippy         # Linting
cargo fmt            # Format
cargo test           # Unit tests (no root needed)
cargo test -- --ignored  # Integration tests (needs root + nftables)
```

## Key Architecture Points

- **Event loop**: `src/main.rs` — central `tokio::select!` over `SystemEvent` channel
- **nftables**: `src/nftables.rs` — all updates are atomic flush+add batches via JSON API
- **HTTP API**: `src/api.rs` — axum server on `127.0.0.1:9100`; needed by MCP server
- **MCP server**: `src/mcp_server.rs` — separate binary, JSON-RPC 2.0 over stdio
- **Config**: YAML at `/etc/rust-network-mgr/config.yaml`; reload via `POST /reload`

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `RUST_LOG` | `info` | Log level |
| `RUST_NETWORK_MGR_CONFIG` | (see config.rs) | Config file override |

## Adding New nftables Zones

Edit the YAML config and add `nftables_zone: <name>` to an interface. The manager auto-creates `<name>_ips` (IPv4) and `<name>_ipv6` sets. The `docker` zone is reserved for Docker container IPs.

## HTTP API Quick Reference

```bash
curl http://127.0.0.1:9100/health
curl http://127.0.0.1:9100/status
curl http://127.0.0.1:9100/interfaces
curl http://127.0.0.1:9100/containers
curl -X POST http://127.0.0.1:9100/reload
curl http://127.0.0.1:9100/metrics
```

## MCP Tools (for Claude Desktop / Cursor / Claude Code)

| Tool | Description |
|---|---|
| `get_status` | Full daemon status |
| `get_interfaces` | Interface→IP mapping |
| `get_containers` | Docker container→IP mapping |
| `reload_config` | Trigger config reload |
| `ping_daemon` | Liveness check |

## MCP Resources

| URI | Description |
|---|---|
| `network://status` | Full status JSON |
| `network://interfaces` | Interface state |
| `network://containers` | Container IPs |

## Testing Nftables Locally

```bash
# Check what sets exist
sudo nft list ruleset

# Check docker zone
sudo nft list set inet filter docker_ips
```
