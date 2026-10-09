//! The stand-in MCP server; see `trek_test_fixtures::fake_mcp`.

fn main() {
    trek_test_fixtures::fake_mcp::run(std::env::args().nth(1));
}
