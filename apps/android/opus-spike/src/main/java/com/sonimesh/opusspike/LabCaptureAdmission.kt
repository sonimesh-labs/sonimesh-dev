package com.sonimesh.opusspike

/** Main-thread admission for one interactive capture request. */
internal class LabCaptureAdmission {
    private enum class Phase { Idle, Permission, Notification, Projection, Starting, Service }
    private var phase = Phase.Idle
    private var needsNotification = false
    private var cancelled = false

    fun canBegin(): Boolean = phase == Phase.Idle
    fun awaitingNotification(): Boolean = phase == Phase.Notification
    fun cancellationPending(): Boolean = cancelled

    fun begin(needsPermission: Boolean, needsNotification: Boolean = false): Boolean {
        if (phase != Phase.Idle) return false
        this.needsNotification = needsNotification
        cancelled = false
        phase = if (needsPermission) Phase.Permission else if (needsNotification) Phase.Notification else Phase.Projection
        return true
    }

    fun permissionResult(granted: Boolean): Boolean {
        if (phase != Phase.Permission) return false
        if (finishCancelled()) return false
        phase = if (!granted) Phase.Idle else if (needsNotification) Phase.Notification else Phase.Projection
        return true
    }

    fun notificationResult(granted: Boolean): Boolean {
        if (phase != Phase.Notification) return false
        if (finishCancelled()) return false
        phase = if (granted) Phase.Projection else Phase.Idle
        return true
    }

    fun projectionResult(granted: Boolean): Boolean {
        if (phase != Phase.Projection) return false
        if (finishCancelled()) return false
        phase = if (granted) Phase.Starting else Phase.Idle
        return true
    }

    fun launchFailed() {
        if (phase == Phase.Permission || phase == Phase.Notification || phase == Phase.Projection || phase == Phase.Starting) {
            phase = Phase.Idle
            cancelled = false
        }
    }

    fun cancelPending(): Boolean {
        if (phase != Phase.Permission && phase != Phase.Notification && phase != Phase.Projection) return false
        cancelled = true
        return true
    }

    private fun finishCancelled(): Boolean {
        if (!cancelled) return false
        cancelled = false
        phase = Phase.Idle
        return true
    }

    fun serviceStarted() { phase = Phase.Service }

    fun serviceFinished() {
        if (phase == Phase.Service) phase = Phase.Idle
    }
}
