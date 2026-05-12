class AgenticHarness < Formula
  desc "Native Rust framework and CLI for building software agents"
  homepage "https://github.com/codejunkie99/agentic-harness"
  license "Apache-2.0"
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
