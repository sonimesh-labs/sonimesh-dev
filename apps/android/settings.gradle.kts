import org.gradle.api.initialization.resolve.RepositoriesMode

pluginManagement {
    repositories {
        maven { url = uri("https://edgedl.me.gvt1.com/android/maven2") }
        gradlePluginPortal()
        mavenCentral()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        maven { url = uri("https://edgedl.me.gvt1.com/android/maven2") }
        mavenCentral()
    }
}

rootProject.name = "sonimesh-android"
include(":app")
include(":discovery-spike")
include(":tls-provider-spike")
