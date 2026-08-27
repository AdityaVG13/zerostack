# typed: strict
# frozen_string_literal: true

class Zerostack < Formula
  desc "Daemonless in-process ZeroKernel runtime for AI coding agents"
  homepage "https://github.com/AdityaVG13/zerostack"
  license "MIT"
  head "https://github.com/AdityaVG13/zerostack.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", "--locked", "--path", "crates/zerostack/zero-kernel", "--root", prefix
  end

  test do
    assert_match "ZeroKernel", shell_output("#{bin}/zero-kernel --help")
  end
end
