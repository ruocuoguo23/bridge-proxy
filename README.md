# Network Bridge Proxy

A high-performance HTTP/HTTPS tunnel proxy service written in Rust, designed to provide external internet access capabilities for restricted services, particularly blockchain applications.

## Features

- **HTTP/HTTPS Tunneling**: Full support for both HTTP and HTTPS traffic through CONNECT method tunneling
- **High Performance**: Built with Tokio async runtime and Hyper for maximum throughput
- **Connection Pooling**: Intelligent connection management with configurable idle timeouts
- **Verbose Logging**: Detailed request/response logging for debugging and monitoring
- **Configurable Timeouts**: Customizable connection and request timeouts
- **Blockchain Ready**: Optimized for blockchain services that need reliable external connectivity

## Use Cases

- Providing internet access for containerized blockchain nodes
- Bypassing network restrictions for DeFi applications
- Enabling external API access for smart contract backends
- Creating secure tunnels for cryptocurrency services

## Installation

### Prerequisites

- Rust 1.70+ 
- Cargo package manager

### Build from Source

```bash
git clone <repository-url>
cd network-bridge-proxy
cargo build --release
```

## Usage

### Basic Usage

Start the proxy with default settings:

```bash
cargo run
```

The proxy will start listening on `127.0.0.1:8080` by default.

### Command Line Options

```bash
cargo run -- [OPTIONS]

Options:
  -a, --address <ADDRESS>    Address to listen on [default: 127.0.0.1:8080]
  -v, --verbose              Enable verbose logging
  -t, --timeout <TIMEOUT>    Connection timeout in seconds [default: 10]
  -h, --help                 Print help information
  -V, --version              Print version information
```

### Examples

**Listen on a different port:**
```bash
cargo run -- --address 0.0.0.0:3128
```

**Enable verbose logging:**
```bash
cargo run -- --verbose
```

**Set custom timeout:**
```bash
cargo run -- --timeout 30 --verbose
```

## Configuration

The proxy can be configured through command-line arguments or by modifying the `Config` struct:

| Parameter | Default | Description |
|-----------|---------|-------------|
| `address` | `127.0.0.1:8080` | Listening address and port |
| `verbose` | `false` | Enable detailed logging |
| `timeout` | `10` | Connection timeout in seconds |
| `max_idle_connections` | `100` | Maximum idle connections per host |
| `idle_timeout` | `90` | Idle connection timeout in seconds |

## Client Configuration

### HTTP Proxy

Configure your application to use the proxy for HTTP requests:

```bash
export http_proxy=http://127.0.0.1:8080
export https_proxy=http://127.0.0.1:8080
```

### Application-Specific Configuration

**cURL:**
```bash
curl --proxy http://127.0.0.1:8080 https://api.example.com
```

**Go:**
```go
package main

import (
    "net/http"
    "net/url"
    "crypto/tls"
)

func main() {
    // Configure proxy
    proxyURL, _ := url.Parse("http://127.0.0.1:8080")
    
    // Create HTTP client with proxy
    client := &http.Client{
        Transport: &http.Transport{
            Proxy: http.ProxyURL(proxyURL),
            TLSClientConfig: &tls.Config{
                InsecureSkipVerify: false, // Set to true for testing only
            },
        },
    }
    
    // Make request through proxy
    resp, err := client.Get("https://api.example.com")
    if err != nil {
        panic(err)
    }
    defer resp.Body.Close()
}
```

**Python (requests):**
```python
import requests

proxies = {
    'http': 'http://127.0.0.1:8080',
    'https': 'http://127.0.0.1:8080'
}

response = requests.get('https://api.example.com', proxies=proxies)
```

## Architecture

The proxy operates with the following components:

- **Main Server**: Accepts incoming connections and routes traffic
- **HTTP Handler**: Processes regular HTTP requests and forwards them
- **CONNECT Handler**: Manages HTTPS tunneling through the CONNECT method
- **Tunnel Module**: Provides bidirectional data streaming between client and target

## Security Considerations

- The proxy does not perform any authentication by default
- All traffic is forwarded without modification
- Consider implementing access controls for production use
- Monitor logs for suspicious activity when verbose mode is enabled

## Performance Tuning

For high-traffic scenarios, consider adjusting:

- `max_idle_connections`: Increase for better connection reuse
- `idle_timeout`: Adjust based on your traffic patterns
- `timeout`: Set appropriate values for your network conditions

## Logging

With verbose mode enabled (`--verbose`), the proxy logs:

- Incoming request details (method, URI)
- Connection establishment status
- Tunnel creation and termination
- Error conditions and timeouts

## Troubleshooting

**Connection refused errors:**
- Verify the target service is accessible
- Check firewall settings
- Ensure DNS resolution is working

**Timeout errors:**
- Increase the timeout value with `--timeout`
- Check network latency to target services

**High memory usage:**
- Reduce `max_idle_connections`
- Lower `idle_timeout` values

## License

## Contributing
