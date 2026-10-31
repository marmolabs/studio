---
kind: protect
level: ask
when:
  paths:
    - "**/*.entitlements"
    - "**/*.mobileprovision"
    - "**/*.p12"
    - "**/ExportOptions.plist"
---
Signing and entitlement files decide who can ship the app. Ask the owner before
changing them.
