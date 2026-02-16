#!/bin/bash
# Local CI Check Script — Emulates GitHub Actions CI

# Set colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

echo -e "${BLUE}=== Starting Local CI Checks ===${NC}"

# 1. Tests
echo -e "\n${YELLOW}[1/4] Running Tests...${NC}"
cargo test
if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓ Tests passed${NC}"
else
    echo -e "${RED}✗ Tests failed${NC}"
fi

# 2. Clippy
echo -e "\n${YELLOW}[2/4] Running Clippy Lints...${NC}"
cargo clippy --all-targets -- -D warnings
if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓ Clippy passed${NC}"
else
    echo -e "${RED}✗ Clippy warnings/errors found${NC}"
fi

# 3. Memory Checks
echo -e "\n${YELLOW}[3/4] Running Memory Optimization Checks...${NC}"

# Binary Size (Mac syntax)
echo -e "${BLUE}Building release binary for size check...${NC}"
cargo build --release --quiet
BINARY="target/release/btc-trading-bot"
if [ -f "$BINARY" ]; then
    SIZE=$(stat -f%z "$BINARY")
    SIZE_MB=$(echo "scale=2; $SIZE / 1048576" | bc)
    echo -e "Binary size: ${SIZE_MB} MB"
    if [ "$SIZE" -gt 15728640 ]; then
        echo -e "${RED}✗ Binary size exceeds 15MB limit${NC}"
    else
        echo -e "${GREEN}✓ Binary size within limits${NC}"
    fi
else
    echo -e "${RED}✗ Could not find release binary${NC}"
fi

# Clone Count
CLONE_COUNT=$(grep -rn "open_pos\.clone()\|open_position\.clone()" src/ --include="*.rs" | wc -l | xargs)
echo -e "OpenPosition clones: $CLONE_COUNT (Baseline: 3)"
if [ "$CLONE_COUNT" -gt 3 ]; then
    echo -e "${YELLOW}! Clone count exceeded baseline${NC}"
else
    echo -e "${GREEN}✓ Clone count okay${NC}"
fi

# Redis LTRIM
LTRIM_COUNT=$(grep -rn "\.ltrim(" src/ --include="*.rs" | wc -l | xargs)
if [ "$LTRIM_COUNT" -eq 0 ]; then
    echo -e "${YELLOW}! No LTRIM calls found for Redis lists${NC}"
else
    echo -e "${GREEN}✓ LTRIM check passed${NC}"
fi

# 4. Dependencies
echo -e "\n${YELLOW}[4/4] Checking Dependency Footprint...${NC}"
TOTAL_DEPS=$(cargo tree -e normal 2>/dev/null | wc -l | xargs)
echo -e "Total transitive dependencies: $TOTAL_DEPS"
if [ "$TOTAL_DEPS" -gt 600 ]; then
    echo -e "${YELLOW}! High dependency count${NC}"
else
    echo -e "${GREEN}✓ Dependency footprint okay${NC}"
fi

echo -e "\n${BLUE}=== Local CI Checks Finished ===${NC}"
