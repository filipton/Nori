import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.compose.compiler)
}

android {
    namespace = "dev.nori.music.app"
    compileSdk = 37

    defaultConfig {
        applicationId = "dev.nori.music"
        minSdk = 26
        targetSdk = 36
        versionName = "0.3.3"
        versionCode = 303
        ndk { abiFilters += (project.findProperty("rustTargets") as String? ?: "arm64-v8a,x86_64").split(",") }
    }

    // Release signing: keystore.properties in the repo root (tools/release.sh creates one, with
    // nori-release.jks, on its first run), or KEYSTORE_FILE / KEYSTORE_PASSWORD / KEY_ALIAS /
    // KEY_PASSWORD in the environment. Without either it falls back to the debug key, so
    // `assembleRelease` always gives an installable APK - but one that cannot update a release build.
    val ksProps = Properties().apply {
        val f = rootProject.file("keystore.properties")
        if (f.exists()) f.inputStream().use { load(it) }
    }
    fun ks(name: String, env: String): String? = ksProps.getProperty(name) ?: System.getenv(env)
    val ksFile = ks("storeFile", "KEYSTORE_FILE")?.let { rootProject.file(it) }
    if (ksFile != null && ksFile.exists()) {
        signingConfigs {
            create("release") {
                storeFile = ksFile
                storePassword = ks("storePassword", "KEYSTORE_PASSWORD")
                keyAlias = ks("keyAlias", "KEY_ALIAS") ?: "nori"
                keyPassword = ks("keyPassword", "KEY_PASSWORD") ?: ks("storePassword", "KEYSTORE_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"))
            signingConfig = signingConfigs.findByName("release") ?: signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures { compose = true }

    packaging {
        resources.excludes += "/META-INF/{AL2.0,LGPL2.1}"
    }
}

composeCompiler {
    stabilityConfigurationFiles.add(layout.projectDirectory.file("compose-stability.conf"))
}

dependencies {
    implementation(project(":core"))
    implementation(platform(libs.compose.bom))
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.navigation.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.compose.ui)
    implementation(libs.compose.material3)
    implementation(libs.compose.material.icons)
    implementation(libs.coil.compose)
    implementation(libs.coil.okhttp)
    implementation(libs.androidx.palette)
}
