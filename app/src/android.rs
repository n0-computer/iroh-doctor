//! Android JNI glue. Android has no XDG home and the WebView blocks the
//! long-press paste menu, so the bits the rest of the app takes for granted
//! on desktop/iOS are pulled off the Android `Context` by hand here.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use jni::objects::{JObject, JString};

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
        let path: String = env.get_string(&JString::from(path)).context("read path")?.into();
        Ok(PathBuf::from(path))
    })
}

/// Primary clip coerced to text via `ClipboardManager` -> `ClipData`. `None`
/// when the clipboard is empty or read access is denied (Android 10+ only
/// hands it over while the app is focused, which a Paste tap satisfies).
pub(crate) fn clipboard_text() -> Option<String> {
    with_env(|env, context| {
        let name = env.new_string("clipboard")?;
        let clipboard = env
            .call_method(
                context,
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[(&name).into()],
            )?
            .l()?;
        let clip = env
            .call_method(&clipboard, "getPrimaryClip", "()Landroid/content/ClipData;", &[])?
            .l()?;
        if clip.is_null() {
            return Ok(None);
        }
        let item = env
            .call_method(&clip, "getItemAt", "(I)Landroid/content/ClipData$Item;", &[0i32.into()])?
            .l()?;
        let text = env
            .call_method(
                &item,
                "coerceToText",
                "(Landroid/content/Context;)Ljava/lang/CharSequence;",
                &[context.into()],
            )?
            .l()?;
        if text.is_null() {
            return Ok(None);
        }
        let text = env
            .call_method(&text, "toString", "()Ljava/lang/String;", &[])?
            .l()?;
        let text: String = env.get_string(&JString::from(text))?.into();
        Ok(Some(text))
    })
    .unwrap_or_else(|e| {
        tracing::warn!(err = %e, "reading android clipboard");
        None
    })
}
