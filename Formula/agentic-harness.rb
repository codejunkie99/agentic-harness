class AgenticHarness < Formula
  desc "Native Rust framework and CLI for building software agents"
  homepage "https://github.com/codejunkie99/agentic-harness"
  license "Apache-2.0"
  url "https://github.com/codejunkie99/agentic-harness/archive/refs/tags/v0.1.1.tar.gz"
  sha256 "fde54b09a98bc6e5ba767e68ef5a8592008a047aa7caa29906d5016dfdd9ef19"
  head "https://github.com/codejunkie99/agentic-harness.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", "--locked", "--path", "crates/agentic-harness-cli", "--root", prefix
  end

  test do
    assert_match "agentic-harness", shell_output("#{bin}/agentic-harness --version")
    assert_match "Agentic Harness Wizard", shell_output("#{bin}/agentic-harness tui --plain")
  end
end
