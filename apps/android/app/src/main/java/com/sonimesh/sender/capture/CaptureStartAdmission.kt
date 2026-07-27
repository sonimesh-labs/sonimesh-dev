package com.sonimesh.sender.capture

/** Bounded admission result for a non-stop foreground-service start intent. */
internal enum class CaptureStartAdmission {
    Start,
    IgnoreActiveDuplicate,
    RejectRetiringGeneration,
}

/** Decides whether one service generation may consume a capture start intent. */
internal fun captureStartAdmission(
    teardownStarted: Boolean,
    ownsCaptureLifecycle: Boolean,
): CaptureStartAdmission = when {
    teardownStarted -> CaptureStartAdmission.RejectRetiringGeneration
    ownsCaptureLifecycle -> CaptureStartAdmission.IgnoreActiveDuplicate
    else -> CaptureStartAdmission.Start
}
