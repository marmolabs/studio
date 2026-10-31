---
name: ios-release-checklist
description: Checklist for shipping a new version of an iOS game to TestFlight and the App Store.
---
# iOS release checklist

Work through these in order. Stop and ask the owner where a step needs their
account or a decision.

1. **Green main.** The default branch builds and its checks pass. Nothing
   half-landed.
2. **Version.** Bump the marketing version (`CFBundleShortVersionString`) and
   the build number (`CFBundleVersion`); the build number must be higher than
   any build already uploaded. Keep both in sync wherever the repo stores them.
3. **Release notes.** Write what's new in plain words: what changed for the
   player, not the code. Keep it within the store's length limit.
4. **Screenshots.** Re-capture screenshots for every required device size when
   the UI changed. Check them for clipped text and debug overlays.
5. **Privacy.** Make sure `PrivacyInfo.xcprivacy` lists every required-reason
   API the app or its dependencies use, and the App Privacy answers in App Store
   Connect still match what the app collects. Re-check after adding any SDK.
6. **Signing.** Confirm the signing identity and provisioning profile are valid
   and not about to expire. Do not edit signing files without asking.
7. **Archive and upload.** Build a release archive and upload it to App Store
   Connect.
8. **TestFlight.** Wait for processing, answer the export-compliance question,
   and install the build on a real device. Play through the first launch, a full
   run, and anything the release changed.
9. **Submit.** Attach the build to the version, fill the notes, and submit for
   review. Choose manual or automatic release as the owner prefers.
10. **After approval.** Tag the release commit and record the build number.
