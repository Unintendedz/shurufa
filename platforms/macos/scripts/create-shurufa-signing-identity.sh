#!/usr/bin/env bash
# 在登录钥匙串里建一张本机自签名代码签名证书，供 build-shurufa.sh 固定签名。
# ad-hoc 签名的 designated requirement 是 cdhash，每次构建都变，麦克风、辅助功能和钥匙串授权都会被当成新应用重新索要；
# 用证书签名后 designated requirement 绑定证书，重编、重装后授权照样有效。钥匙串里已有同名证书时什么也不做。
set -euo pipefail
name="${SHURUFA_SIGNING_NAME:-Shurufa Local Signing}"
keychain="$HOME/Library/Keychains/login.keychain-db"
if security find-certificate -c "$name" "$keychain" >/dev/null 2>&1; then
  echo "钥匙串里已有「$name」"
  exit 0
fi
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cat > "$work/cert.cnf" <<EOF
[req]
distinguished_name = dn
prompt = no
x509_extensions = ext
[dn]
CN = $name
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
EOF
# 用系统自带的 LibreSSL：它导出的 PKCS#12 用 security import 认得的旧算法，Homebrew 的 OpenSSL 3 默认导出的 security 读不了。
/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -config "$work/cert.cnf" \
  -keyout "$work/key.pem" -out "$work/cert.pem" 2>/dev/null
password="$(/usr/bin/openssl rand -hex 16)"
/usr/bin/openssl pkcs12 -export -inkey "$work/key.pem" -in "$work/cert.pem" -name "$name" \
  -passout "pass:$password" -out "$work/identity.p12"
# -T 让 codesign 用私钥时不再逐次弹窗。
security import "$work/identity.p12" -k "$keychain" -P "$password" -T /usr/bin/codesign
security find-certificate -c "$name" -Z "$keychain" | awk '/^SHA-1 hash:/{print "已建「'"$name"'」，SHA-1 " $3}'
