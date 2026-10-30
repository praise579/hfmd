class Hfmd < Formula
  desc "Hugging Face model downloader with segmented resume and hash verification"
  homepage "https://github.com/praise579/hfmd"
  version "0.1.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/praise579/hfmd/releases/download/v#{version}/hfmd-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_SHA256"
    else
      url "https://github.com/praise579/hfmd/releases/download/v#{version}/hfmd-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_SHA256"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/praise579/hfmd/releases/download/v#{version}/hfmd-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_SHA256"
    else
      url "https://github.com/praise579/hfmd/releases/download/v#{version}/hfmd-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_SHA256"
    end
  end

  def install
    bin.install "hfmd"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/hfmd --version")
  end
end
