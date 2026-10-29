//! The page-to-host channel of the cosmetic filtering script: a WebView2 host
//! object whose `generic(documentUrl, json)` method answers with the generic
//! hiding selectors for the classes and ids a document reports. Calls travel
//! over WebView2's own messaging, so they are not network requests and never
//! appear in the page's Resource Timing or PerformanceObserver entries. The
//! page cannot reach the object: the document-start script hands it to Jello's
//! own script and removes it before any page script runs (see bootstrap.js).

#![cfg(target_os = "windows")]

use std::mem::ManuallyDrop;
use windows::core::{implement, Result, BSTR, GUID, PCWSTR};
use windows::Win32::Foundation::{DISP_E_BADPARAMCOUNT, DISP_E_MEMBERNOTFOUND, DISP_E_UNKNOWNNAME, E_NOTIMPL};
use windows::Win32::System::Com::{IDispatch, IDispatch_Impl, ITypeInfo, DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO};
use windows::Win32::System::Variant::{VARIANT, VT_BSTR, VT_DISPATCH};

const GENERIC: i32 = 1;

/// Answers `generic(documentUrl, json)` with a JSON array of selectors.
pub type Reply = Box<dyn Fn(&str, &str) -> String>;

#[implement(IDispatch)]
struct Channel {
    reply: Reply,
}

impl IDispatch_Impl for Channel_Impl {
    fn GetTypeInfoCount(&self) -> Result<u32> {
        Ok(0)
    }

    fn GetTypeInfo(&self, _index: u32, _lcid: u32) -> Result<ITypeInfo> {
        Err(E_NOTIMPL.into())
    }

    fn GetIDsOfNames(&self, _riid: *const GUID, names: *const PCWSTR, count: u32, _lcid: u32, ids: *mut i32) -> Result<()> {
        let mut known = true;
        for i in 0..count as usize {
            // SAFETY: the caller passes `count` names and room for `count` ids.
            let name = unsafe { (*names.add(i)).to_string() }.unwrap_or_default();
            let id = if name == "generic" {
                GENERIC
            } else {
                known = false;
                -1
            };
            unsafe { *ids.add(i) = id };
        }
        if known {
            Ok(())
        } else {
            Err(DISP_E_UNKNOWNNAME.into())
        }
    }

    fn Invoke(
        &self,
        id: i32,
        _riid: *const GUID,
        _lcid: u32,
        _flags: DISPATCH_FLAGS,
        params: *const DISPPARAMS,
        result: *mut VARIANT,
        _excepinfo: *mut EXCEPINFO,
        _argerr: *mut u32,
    ) -> Result<()> {
        if id != GENERIC {
            return Err(DISP_E_MEMBERNOTFOUND.into());
        }
        // SAFETY: `params` comes from the dispatch caller.
        let args = unsafe { string_args(params) };
        let [url, data] = args.as_slice() else {
            return Err(DISP_E_BADPARAMCOUNT.into());
        };
        let reply = (self.reply)(url, data);
        if !result.is_null() {
            unsafe { *result = bstr_variant(&reply) };
        }
        Ok(())
    }
}

/// Positional string arguments in call order (DISPPARAMS lists them last
/// first); anything that is not a string reads as "".
unsafe fn string_args(params: *const DISPPARAMS) -> Vec<String> {
    if params.is_null() {
        return Vec::new();
    }
    let params = &*params;
    (0..params.cArgs as usize)
        .rev()
        .map(|i| {
            let value = &(*params.rgvarg.add(i)).Anonymous.Anonymous;
            if value.vt == VT_BSTR {
                value.Anonymous.bstrVal.to_string()
            } else {
                String::new()
            }
        })
        .collect()
}

fn bstr_variant(text: &str) -> VARIANT {
    let mut v = VARIANT::default();
    unsafe {
        let value = &mut *v.Anonymous.Anonymous;
        value.vt = VT_BSTR;
        value.Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(text));
    }
    v
}

/// A new channel answering with `reply`.
pub fn new(reply: Reply) -> IDispatch {
    Channel { reply }.into()
}

/// The channel as a VARIANT for `AddHostObjectToScript` (one per document or
/// frame it is added to). The caller clears it afterwards; WebView2 keeps its
/// own reference.
pub fn variant(channel: &IDispatch) -> VARIANT {
    let mut v = VARIANT::default();
    unsafe {
        let value = &mut *v.Anonymous.Anonymous;
        value.vt = VT_DISPATCH;
        value.Anonymous.pdispVal = ManuallyDrop::new(Some(channel.clone()));
    }
    v
}
