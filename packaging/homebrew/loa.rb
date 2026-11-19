# Homebrew formula template. Replace OWNER/REPO, VERSION and the sha256 values.
class Loa < Formula
  desc "Unofficial CLI for LINE Developers console and LINE Official Account setup"
  homepage "https://github.com/OWNER/REPO"
  version "VERSION"
  license "AGPL-3.0-only"

  on_arm do
    url "https://github.com/OWNER/REPO/releases/download/vVERSION/loa-VERSION-aarch64-apple-darwin.tar.gz"
    sha256 "SHA256_AARCH64"
  end
  on_intel do
    url "https://github.com/OWNER/REPO/releases/download/vVERSION/loa-VERSION-x86_64-apple-darwin.tar.gz"
    sha256 "SHA256_X86_64"
  end

  def install
    bin.install "loa"
  end

  def caveats
    <<~EOS
      loa drives a browser through agent-browser (not bundled):
        npm install -g agent-browser
        loa setup
      loa is not affiliated with LY Corporation. Check each service's terms before use.
    EOS
  end

  test do
    assert_match "USAGE", shell_output("#{bin}/loa --json schema nope", 2)
  end
end
