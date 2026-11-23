package com.sonimesh.opusspike

/** Main-thread admission for one interactive capture request. */
internal class LabCaptureAdmission {
    private enum class Phase { Idle, Permission, Projection, Starting, Service }
    private var phase = Phase.Idle

    fun canBegin(): Boolean = phase == Phase.Idle

    fun begin(needsPermission: Boolean): Boolean {
        if (phase != Phase.Idle) return false
        phase = if (needsPermission) Phase.Permission else Phase.Projection
        return true
    }

    fun permissionResult(granted: Boolean): Boolean {
        if (phase != Phase.Permission) return false
        phase = if (granted) Phase.Projection else Phase.Idle
        return true
    }

    fun projectionResult(granted: Boolean): Boolean {
        if (phase != Phase.Projection) return false
        phase = if (granted) Phase.Starting else Phase.Idle
        return true
    }

    fun launchFailed() {
        if (phase == Phase.Permission || phase == Phase.Projection || phase == Phase.Starting) {
            phase = Phase.Idle
        }
    }

    fun serviceStarted() { phase = Phase.Service }

    fun serviceFinished() {
        if (phase == Phase.Service) phase = Phase.Idle
    }
}
