//! Android JNI glue. Android has no XDG home and the WebView blocks the
//! long-press paste menu, so the bits the rest of the app takes for granted
//! on desktop/iOS are pulled off the Android `Context` by hand here.

use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context as _, Result};
use jni::objects::{JObject, JString};
use tokio::sync::mpsc;

/// Runs `f` with a JNI env attached to the current thread and the Android
/// `Context`. UI event handlers and the early path setup both run on the
/// main thread, so the binder calls inside (clipboard, getFilesDir) are safe.
fn with_env<T>(f: impl FnOnce(&mut jni::JNIEnv, &JObject) -> Result<T>) -> Result<T> {
    let ctx = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }.context("android JavaVM")?;
    let mut env = vm.attach_current_thread().context("attach JNI thread")?;
    let context = unsafe { JObject::from_raw(ctx.context().cast()) };
    f(&mut env, &context)
}

/// `Context.getFilesDir()` - the app's private storage root, our stand-in for
/// `dirs::config_dir()`.
pub(crate) fn files_dir() -> Result<PathBuf> {
    with_env(|env, context| {
        let dir = env
            .call_method(context, "getFilesDir", "()Ljava/io/File;", &[])
            .context("Context.getFilesDir")?
            .l()?;
        let path = env
            .call_method(&dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
            .context("File.getAbsolutePath")?
            .l()?;
        let path: String = env
            .get_string(&JString::from(path))
            .context("read path")?
            .into();
        Ok(PathBuf::from(path))
    })
}

/// The URL iroh-doctor was launched with via an `irohdoctor://` deep link, or
/// `None` for a normal launch. The `ndk_context` context is the `WryActivity`,
/// so `getIntent().getData()` returns the launch intent's URI. Read once at
/// startup: tao does not forward a fresh intent to a running app (wry #1563),
/// so a scan while the app is open is handled by the `onNewIntent` glue instead.
pub(crate) fn launch_deep_link() -> Option<String> {
    with_env(|env, context| {
        let intent = env
            .call_method(context, "getIntent", "()Landroid/content/Intent;", &[])
            .context("Activity.getIntent")?
            .l()?;
        let uri = env
            .call_method(&intent, "getData", "()Landroid/net/Uri;", &[])
            .context("Intent.getData")?
            .l()?;
        // A normal launch has no data URI; only a deep link sets one.
        if uri.is_null() {
            return Ok(None);
        }
        let text = env
            .call_method(&uri, "toString", "()Ljava/lang/String;", &[])
            .context("Uri.toString")?
            .l()?;
        let text: String = env
            .get_string(&JString::from(text))
            .context("read uri")?
            .into();
        Ok(Some(text))
    })
    .unwrap_or_else(|e| {
        tracing::warn!(err = %e, "reading android launch intent");
        None
    })
}

/// Sender for the deep-link channel, replaced every time [`deep_link_channel`]
/// hands out a fresh receiver so that a remount of the UI re-registers a live
/// sender rather than orphaning its new receiver. Held in a global because the
/// JNI callback runs with no app context through which to reach a Dioxus signal.
static DEEP_LINK_TX: Mutex<Option<mpsc::Sender<String>>> = Mutex::new(None);

/// Creates the deep-link receiver and registers its sender for the JNI
/// callback, replacing any previous sender. The receiver streams every deep
/// link the app receives while running.
pub(crate) fn deep_link_channel() -> mpsc::Receiver<String> {
    // Deep links are user-paced, so no backlog builds; the small bound just
    // drops (try_send) rather than blocking the Android main thread if the UI
    // is momentarily behind.
    let (tx, rx) = mpsc::channel(4);
    *DEEP_LINK_TX.lock().expect("poisoned") = Some(tx);
    rx
}

/// Forwards a non-empty deep-link URL to the current [`deep_link_channel`]
/// receiver, if one is registered. Used to seed the cold-start launch intent;
/// the JNI callback calls it for warm-start intents.
pub(crate) fn push_deep_link(url: String) {
    if url.is_empty() {
        return;
    }
    let tx = DEEP_LINK_TX.lock().expect("poisoned").clone();
    if let Some(tx) = tx {
        let _ = tx.try_send(url);
    }
}

/// JNI entry point for `MainActivity.onNewIntent`, patched into the generated
/// Kotlin by `scripts/bundle-mobile.sh`. Receives the new intent's data URI (an
/// empty string when it has none) and forwards it to the running app. Runs on
/// the Android main thread. tao does not surface new intents itself (wry
/// #1563), so this callback is how a scan reaches an already-running app.
#[no_mangle]
pub extern "system" fn Java_dev_dioxus_main_MainActivity_newDeepLink(
    mut env: jni::JNIEnv,
    _this: JObject,
    url: JString,
) {
    // A panic must not unwind across the JNI boundary. Nothing here panics
    // today, but the guard keeps a future dependency change from turning a
    // deep link into a process abort.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        match env.get_string(&url) {
            Ok(url) => push_deep_link(url.into()),
            Err(e) => tracing::warn!(err = %e, "reading onNewIntent url"),
        }
    }));
}
