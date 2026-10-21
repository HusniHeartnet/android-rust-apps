# Signing

APKs must be signed before Android installs them. The key is yours and never goes in this repo.

```
keytool -genkeypair -v -keystore heartnet-release.keystore -alias heartnet -keyalg RSA -keysize 4096 -validity 10000
```

Keep the keystore and its password somewhere safe and backed up: if the key is lost, installed
apps can no longer be updated. Then, in the shell that builds:

```
$env:HEARTNET_KEYSTORE = 'C:\path\to\heartnet-release.keystore'
$env:HEARTNET_KEYSTORE_PASS = '<the password>'
```

Release notes should include the certificate fingerprint (`apksigner verify --print-certs app.apk`)
so people can check what they download.
