# Template only. The formula users actually install lives in
# marcmantei/homebrew-tap and is regenerated on every release by
# .github/workflows/update-homebrew.yml, which fills in the real version and
# checksums. The version and PLACEHOLDER digests below are illustrative — do not
# expect this file to install.
class Myc < Formula
  desc "Code knowledge graph engine for AI agents"
  homepage "https://github.com/marcmantei/myceliums"
  version "0.3.1"
  license "Apache-2.0"

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
