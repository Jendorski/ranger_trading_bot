# BTC Trading Bot

![CI — Tests & Memory Checks](https://github.com/Jendorski/ranger_trading_bot/actions/workflows/ci.yml/badge.svg)

A sophisticated Rust-based automated cryptocurrency trading bot for Bitcoin futures trading on the Bitget exchange. The bot implements advanced technical analysis strategies including Smart Money Concepts (SMC) and Ichimoku Cloud indicators to identify high-probability trade setups with automated risk management.

## 🎯 Overview

This trading bot is designed for automated algorithmic trading of BTC/USDT perpetual futures contracts. It combines multiple technical analysis frameworks, zone-based trade management, and sophisticated risk control to execute systematic trading strategies. The bot operates continuously, monitoring market conditions and executing trades based on predefined rules while maintaining detailed performance analytics.

## ✨ Key Features

### Trading Strategies
- **Smart Money Concepts (SMC)**: Identifies market structure including:
  - Break of Structure (BOS) and Change of Character (CHOCH) detection
  - Order Block identification
  - Liquidity sweeps (sweep highs/lows)
  - Strong High/Low detection for entry signals
  - Configurable timeframes (15m, 4H, 1D, etc.)

- **Ichimoku Cloud Indicator**: Weekly timeframe analysis for:
  - Tenkan-Sen / Kijun-Sen crossover signals
  - Kumo (cloud) cross detection
  - Long-term trend confirmation
  - Historical Bitcoin data processing

### Risk Management
- **Zone-Based Trading**: Trade only in validated price zones
  - Zone cooldown system (prevents overtrading)
  - Loss tracking per zone (max consecutive losses)
  - Automatic zone filtering by minimum distance
  - Zone overlap detection and conflict resolution

- **Position Management**:
  - Configurable leverage (default: 20x)
  - Risk percentage per trade (default: 5%)
  - Stop-loss automation
  - Partial profit targets with adjustable quantities
  - Dynamic position sizing based on account balance

### Exchange Integration
- **Bitget Futures API**: Full integration with Bitget derivatives platform
  - Market order execution (long/short)
  - Stop-loss and take-profit modification
  - Real-time price feeds via REST API
  - WebSocket candlestick data streaming
  - Funding rate monitoring
  - VIP fee tier support

### Data & Analytics
- **Redis-Based Persistence**: 
  - Position state management
  - Trade history storage
  - Zone statistics tracking
  - Indicator data caching
  - Performance metrics

- **Performance Analytics**:
  - Cumulative ROI calculations (weekly/monthly)
  - PnL tracking per trade
  - Win/loss ratio analysis
  - Fee calculations (maker/taker)
  - Trade execution logs

### Technical Infrastructure
- **Asynchronous Architecture**: Built with Tokio runtime for concurrent operations
- **WebSocket Streaming**: Real-time market data subscriptions
- **Docker Support**: Containerized deployment with Redis orchestration
- **Configurable Scheduling**: Cron-based indicator updates
- **Comprehensive Logging**: Structured logging with configurable levels

## 🏗️ Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                         Main Application                     │
│                         (main.rs)                            │
└────────────┬──────────────────────────────────┬─────────────┘
             │                                  │
             ▼                                  ▼
    ┌────────────────┐              ┌──────────────────────┐
    │  Bot Engine    │              │   Tracker Modules    │
    │  (bot/mod.rs)  │              │   (trackers/)        │
    │                │              │                      │
    │ • Position Mgmt│              │ • SMC Engine         │
    │ • Entry/Exit   │◄────────────►│ • Ichimoku           │
    │ • Stop Loss    │              │ • Momentum           │
    │ • Partial TP   │              │                      │
    └────────┬───────┘              └──────────┬───────────┘
             │                                  │
             ▼                                  ▼
    ┌────────────────┐              ┌──────────────────────┐
    │  Zone Guard    │              │   Redis Cache        │
    │ (bot/zones/)   │◄────────────►│   (cache/mod.rs)     │
    │                │              │                      │
    │ • Zone Stats   │              │ • State Persistence  │
    │ • Cooldowns    │              │ • Trade History      │
    │ • Loss Limits  │              │ • Analytics Data     │
    └────────────────┘              └──────────────────────┘
             │
             ▼
    ┌────────────────────────────────────────────────────────┐
    │              Exchange Interface (exchange/)             │
    │                                                         │
    │  ┌──────────────┐    ┌──────────────┐   ┌──────────┐ │
    │  │   REST API   │    │   WebSocket  │   │   Fees   │ │
    │  │              │    │              │   │          │ │
    │  │ • Prices     │    │ • Tickers    │   │ • VIP 0-9│ │
    │  │ • Orders     │    │ • Candles    │   │ • Calc   │ │
    │  │ • Funding    │    │ • Real-time  │   │          │ │
    │  └──────────────┘    └──────────────┘   └──────────┘ │
    └────────────────────────────────────────────────────────┘
                             │
                             ▼
                    ┌────────────────┐
                    │ Bitget Exchange│
                    │   (Live API)   │
                    └────────────────┘
```

## 🚀 Getting Started

### Prerequisites

- **Rust**: Version 1.83 or later ([install](https://rustup.rs/))
- **Docker & Docker Compose**: For containerized deployment ([install](https://docs.docker.com/get-docker/))
- **Redis**: Version 7+ (provided via Docker Compose)
- **Bitget Account**: With API credentials and futures trading enabled

### Installation

#### Option 1: Docker (Recommended)

1. **Clone the repository**:
   ```bash
   git clone <repository-url>
   cd btc_trading_bot
   ```

2. **Configure environment** (see [Configuration](#-configuration)):
   ```bash
   cp .env.example .env
   nano .env  # Edit with your API keys
   ```

3. **Build and run**:
   ```bash
   docker-compose up --build
   ```

#### Option 2: Local Development

1. **Clone and navigate**:
   ```bash
   git clone <repository-url>
   cd btc_trading_bot
   ```

2. **Install Redis** (macOS):
   ```bash
   brew install redis
   brew services start redis
   ```

3. **Configure environment**:
   ```bash
   cp .env.example .env
   nano .env
   ```

4. **Build and run**:
   ```bash
   cargo build --release
   cargo run --release
   ```

## ⚙️ Configuration

Create a `.env` file in the project root with the following parameters:

### Required Configuration

```bash
# Bitget API Credentials (REQUIRED)
API_KEY=your_bitget_api_key_here
API_SECRET=your_bitget_api_secret_here
ACCESS_PASSPHRASE=your_bitget_passphrase_here

# Redis Connection (REQUIRED)
REDIS_URL=redis://127.0.0.1:6379  # or redis://redis:6379 for Docker

# Trading Symbol (REQUIRED)
SYMBOL=BTCUSDT

# Indicator Toggles (REQUIRED)
USE_SMC_INDICATOR=true        # Enable Smart Money Concepts
USE_ICHIMOKU_INDICATOR=true   # Enable Ichimoku Cloud
```

### Trading Parameters

```bash
# Capital & Risk Management
MARGIN=50.00                  # Initial margin in USDT
LEVERAGE=20.00                # Leverage multiplier (1-125)
RISK_PERCENTAGE=0.05          # Risk per trade (5% of margin)
RANGER_RISK_PERCENTAGE=0.075  # Risk for ranger trades (7.5%)

# Zone Configuration
RANGER_PRICE_DIFFERENCE=1750.0  # Minimum zone separation in USD

# Bot Settings
POLL_INTERVAL_SECS=3          # Market polling frequency
```

### Smart Money Concepts (SMC) Settings

```bash
# SMC Indicator Configuration
SMC_TIMEFRAME=4H              # Options: 15m, 30m, 1h, 4H, 12h, 1d, 1w
SMC_CANDLE_COUNT=150          # Number of historical candles to analyze
                              # Recommended: 150 for 4H, 333 for 15m, 1000 for 1d
```

**Timeframe Guidelines**:
- `15m` + `333 candles`: Short-term, frequent signals (intraday)
- `4H` + `150 candles`: Medium-term, balanced approach (swing)
- `1d` + `1000 candles`: Long-term, high-conviction setups

### Ichimoku Settings

The Ichimoku indicator uses weekly Bitcoin historical data. Configuration is hardcoded to standard parameters:
- **Tenkan-Sen (Conversion)**: 9 periods
- **Kijun-Sen (Base)**: 26 periods
- **Senkou Span B**: 52 periods
- **Displacement**: 26 periods

Data source: [Kaggle Bitcoin Historical Dataset](https://www.kaggle.com/datasets/mczielinski/bitcoin-historical-data)

### Configuration Tips

> [!IMPORTANT]
> **Never commit your `.env` file** to version control. It contains sensitive API credentials.

> [!WARNING]
> **Start with small margin and conservative risk settings** when first deploying. Test thoroughly on paper trading or with minimal capital.

> [!TIP]
> For high-frequency signals, use `15m` timeframe with `SMC_CANDLE_COUNT=333`. For conservative swing trading, use `4H` or `1d` with higher candle counts.

## 📋 Usage

### Running the Bot

**With Docker**:
```bash
docker-compose up
```

**Without Docker**:
```bash
# Ensure Redis is running
redis-server

# Run the bot
cargo run --release
```

### Monitoring

The bot outputs structured logs to stdout:

```
[INFO] Starting bot loop...
[INFO] SMC Tracker initialized with timeframe: 4H
[INFO] Ichimoku Tracker started
[INFO] Current position: Flat
[INFO] Found StrongLow zone at 45250.0 - 45300.0
[INFO] Entering LONG position at 45275.0
[INFO] Stop-loss set at 45100.0
[INFO] Partial profit target 1: 45750.0 (50%)
```

### Interpreting Bot Behavior

1. **Indicator Trackers**: Run on separate async tasks, updating zones in Redis
2. **Main Bot Loop**: Polls price every `POLL_INTERVAL_SECS`, checks for entry/exit conditions
3. **Zone Guard**: Prevents trading in zones with too many losses or recent activity
4. **Position Management**: Automatically adjusts stop-loss, takes partial profits, closes positions

### Testing

Run the included test suite:

```bash
# Run all tests
cargo test

# Run specific module tests
cargo test --package btc-trading-bot --lib exchange::bitget::tests

# Run with output
cargo test -- --nocapture
```

## 📁 Project Structure

```
btc_trading_bot/
├── src/
│   ├── main.rs                    # Application entry point
│   ├── bot/
│   │   ├── mod.rs                 # Core trading logic (1331 lines)
│   │   │                          # • Position management
│   │   │                          # • Entry/exit logic
│   │   │                          # • Stop-loss & partial profits
│   │   ├── scalper/               # [Legacy] Scalping strategy module
│   │   └── zones/
│   │       └── mod.rs             # Zone-based trade management
│   │                              # • ZoneGuard: cooldowns & loss limits
│   │                              # • Zone validation & filtering
│   ├── cache/
│   │   └── mod.rs                 # Redis client wrapper
│   ├── config/
│   │   └── mod.rs                 # Environment configuration
│   ├── encryption/
│   │   └── mod.rs                 # HMAC-SHA256 signing for API
│   ├── exchange/
│   │   ├── mod.rs                 # Exchange trait definition
│   │   └── bitget/
│   │       ├── mod.rs             # Bitget REST/WebSocket API
│   │       │                      # • Price feeds
│   │       │                      # • Order execution
│   │       │                      # • Candlestick streaming
│   │       └── fees/
│   │           └── mod.rs         # Fee calculation (VIP tiers)
│   ├── graph/
│   │   └── mod.rs                 # Performance analytics
│   │                              # • ROI calculations
│   │                              # • Weekly/monthly aggregation
│   ├── helper/
│   │   └── mod.rs                 # Utility functions & constants
│   └── trackers/
│       ├── mod.rs                 # Tracker module exports
│       ├── smart_money_concepts/
│       │   └── mod.rs             # SMC indicator engine (783 lines)
│       │                          # • Pivot detection
│       │                          # • BOS/CHOCH logic
│       │                          # • Strong High/Low signals
│       ├── ichimoku/
│       │   └── mod.rs             # Ichimoku Cloud (340 lines)
│       │                          # • Weekly timeframe processing
│       │                          # • Kumo cross detection
│       └── momentum/              # [Future] Momentum indicators
│           └── mod.rs
├── Cargo.toml                     # Rust dependencies
├── Dockerfile                     # Multi-stage container build
├── docker-compose.yml             # Services orchestration (app + Redis)
├── .env.example                   # Environment template
├── .gitignore
├── CANDLESTICK_WEBSOCKET_WALKTHROUGH.md  # WebSocket implementation guide
└── data/                          # [Excluded] Historical data cache
```

### Key Modules Explained

| Module | Purpose | Key Files |
|--------|---------|-----------|
| **bot** | Core trading engine | `mod.rs`, `zones/mod.rs` |
| **exchange** | Bitget API integration | `bitget/mod.rs`, `bitget/fees/mod.rs` |
| **trackers** | Technical indicators | `smart_money_concepts/mod.rs`, `ichimoku/mod.rs` |
| **cache** | Redis persistence | `mod.rs` |
| **graph** | Analytics & reporting | `mod.rs` |
| **config** | Environment management | `mod.rs` |
| **encryption** | API authentication | `mod.rs` |

## 📦 Dependencies

### Core Runtime
- **tokio** `1.x`: Async runtime with full features
- **tokio-cron-scheduler** `0.13`: Job scheduling for indicators
- **async-trait** `0.1`: Trait support for async functions
- **anyhow** `1.0`: Error handling with context

### Networking
- **reqwest** `0.11`: HTTP client (JSON, blocking, streaming)
- **tokio-tungstenite** `0.28`: WebSocket client
- **futures-util** `0.3`: Stream combinators
- **url** `2.x`: URL parsing

### Serialization
- **serde** `1.x`: Serialization framework
- **serde_json** `1.x`: JSON support

### Database
- **redis** `0.23`: Async Redis client
- **redis-derive** `0.1`: Custom Redis types

### Cryptography
- **hmac** `0.12`: HMAC implementation
- **sha2** `0.10`: SHA-256 hashing
- **base64** `0.22`: Base64 encoding
- **digest** `0.10`: Generic hash functions

### Data Processing
- **csv** `1.3`: CSV parsing (for Ichimoku data)
- **zip** `7.0`: ZIP archive extraction
- **chrono** `0.4`: Date/time handling

### Utilities
- **rust_decimal**: Precise decimal arithmetic for financial calculations
- **uuid** `1.4`: Unique identifiers for trades
- **dotenv** `0.15`: `.env` file loading
- **log** `0.4` + **simple_logger** `1.13`: Logging infrastructure

## 🔧 Development

### Building

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Check without building
cargo check
```

### Running Tests

```bash
# All tests
cargo test

# Specific module
cargo test exchange::bitget

# With log output
cargo test -- --nocapture --test-threads=1
```

### Code Quality

```bash
# Format code
cargo fmt

# Lint
cargo clippy

# Clippy with pedantic warnings
cargo clippy -- -W clippy::pedantic
```

### Docker Development

```bash
# Build image
docker build -t btc-trading-bot .

# Run with compose
docker-compose up --build

# View logs
docker-compose logs -f app

# Stop services
docker-compose down
```

## 🐛 Troubleshooting

### Common Issues

**1. Redis Connection Failed**
```
Error: Connection refused (os error 111)
```
**Solution**: Ensure Redis is running on the configured `REDIS_URL`.
```bash
# Local: Start Redis
redis-server

# Docker: Check Redis health
docker-compose logs redis
```

**2. API Authentication Error**
```
Error: Invalid signature
```
**Solution**: Verify your `.env` has correct `API_KEY`, `API_SECRET`, and `ACCESS_PASSPHRASE`.

**3. WebSocket Disconnection**
```
WebSocket connection closed
```
**Solution**: Bot auto-reconnects. If persistent, check network stability and Bitget API status.

**4. No Trading Signals**
```
INFO: Current position: Flat (no zones found)
```
**Solution**: 
- SMC needs sufficient market structure. Wait for BOS/CHOCH.
- Check `SMC_CANDLE_COUNT` is appropriate for your `SMC_TIMEFRAME`.
- Verify zones aren't filtered due to `RANGER_PRICE_DIFFERENCE` being too large.

**5. Build Errors**
```
error: failed to compile btc-trading-bot
```
**Solution**: Ensure Rust version is 1.83+:
```bash
rustup update stable
cargo clean
cargo build
```

## 🛡️ Risk Disclaimer

> [!CAUTION]
> **This software is for educational and experimental purposes only.**
> 
> - Cryptocurrency trading involves substantial risk of loss.
> - This bot does NOT guarantee profits and can lose money.
> - Past performance does not indicate future results.
> - Always start with small amounts and paper trading.
> - Never trade with funds you cannot afford to lose.
> - The authors are not responsible for any financial losses.

**USE AT YOUR OWN RISK.**

## 📄 License

This project is provided as-is without warranty. See `LICENSE` file for details (if applicable).

## 🤝 Contributing

Contributions, issues, and feature requests are welcome! Feel free to check the issues page or submit pull requests.

### Development Priorities
- [ ] Add backtesting framework
- [ ] Implement paper trading mode
- [ ] Add more exchange integrations (Binance, Bybit)
- [ ] Telegram notification system
- [ ] Web-based dashboard for monitoring
- [ ] Strategy optimization tools

## 📚 Additional Resources

- [Bitget API Documentation](https://bitgetlimited.github.io/apidoc/en/mix/)
- [Smart Money Concepts Guide](https://www.investopedia.com/smart-money-concepts)
- [Ichimoku Cloud Explained](https://www.investopedia.com/terms/i/ichimoku-cloud.asp)
- [Candlestick WebSocket Implementation](./CANDLESTICK_WEBSOCKET_WALKTHROUGH.md)

## 📞 Support

For questions or support:
- Open an issue on GitHub
- Review the [CANDLESTICK_WEBSOCKET_WALKTHROUGH.md](./CANDLESTICK_WEBSOCKET_WALKTHROUGH.md) for WebSocket implementation details
- Check the conversation history for common solutions

---

**Built with ❤️ using Rust • Powered by Tokio • Trading on Bitget**
