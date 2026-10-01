plugins {
    id("com.android.application")
}

android {
    namespace = "com.waterkit.test"
    compileSdk = 37

    defaultConfig {
        applicationId = "com.waterkit.test"
        minSdk = 26
        targetSdk = 37
        versionCode = 1
        versionName = "1.0"

        ndk {
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets {
        getByName("main") {
            jniLibs.directories.add("src/main/jniLibs")
            // Every waterkit crate declares its Android Kotlin helpers under
            // `src/sys/android` (see its [package.metadata.waterui.android]
            // manifest entries); they compile into this app's DEX and are
            // resolved at run time through the application ClassLoader.
            val waterkitRoot = rootProject.projectDir.parentFile.parentFile
            waterkitRoot.listFiles()?.forEach { group ->
                group.listFiles()?.forEach { crate ->
                    val androidSources = File(crate, "src/sys/android")
                    if (androidSources.isDirectory) {
                        java.srcDir(androidSources)
                    }
                }
            }
        }
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.19.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("com.google.android.material:material:1.14.0")
    implementation("androidx.activity:activity-ktx:1.13.0")
    // Vendored jars waterkit-health's Kotlin helper compiles against
    // ([package.metadata.waterui.android] jars entries).
    implementation(
        fileTree(File(rootProject.projectDir.parentFile.parentFile, "device/health/third_party")) {
            include("**/*.jar")
        }
    )
}
