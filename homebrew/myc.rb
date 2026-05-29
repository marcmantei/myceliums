class Myc < Formula
  desc "Code knowledge graph engine for AI agents"
  homepage "https://myceliums.ai"
  version "0.2.0"
  license "AGPL-3.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/marcmantei/myceliums/releases/download/v#{version}/myc-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER"
    else
      url "https://github.com/marcmantei/myceliums/releases/download/v#{version}/myc-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/marcmantei/myceliums/releases/download/v#{version}/myc-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER"
    else
      url "https://github.com/marcmantei/myceliums/releases/download/v#{version}/myc-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER"
    end
  end

  def install
    bin.install "myc"
  end

  test do
    assert_match "myc", shell_output("#{bin}/myc --version")
  end
end
