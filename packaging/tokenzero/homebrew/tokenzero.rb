class Tokenzero < Formula
  desc "Recovery-aware context compression runtime for AI coding agents"
  homepage "https://github.com/AdityaVG13/tokenzero"
  version "1.4.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/AdityaVG13/tokenzero/releases/download/v1.4.0/tokenzero-v1.4.0-aarch64-apple-darwin.tar.gz"
      sha256 "8358f590c9d15173cf30a9e8967fe8731bb636820171a3b69042c1460c58c481"
    else
      url "https://github.com/AdityaVG13/tokenzero/releases/download/v1.4.0/tokenzero-v1.4.0-x86_64-apple-darwin.tar.gz"
      sha256 "f378b8938dfa6ca795df3b10cd0fdb99d8e4a0bbaf436eb8cd95eba9949b680b"
    end
  end

  def install
    bin.install "tokenzero"
  end

  test do
    assert_match "tokenzero", shell_output("#{bin}/tokenzero --version")
    system "#{bin}/tokenzero", "doctor", "--json"
  end
end
