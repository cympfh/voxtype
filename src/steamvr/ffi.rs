//! OpenVR プレーンC API (openvr_api.dll) への生FFIバインディング。
//!
//! 公式OpenVR C++ SDKはMSVCビルドでvtable ABIがこのプロジェクトのクロスコンパイル先
//! (x86_64-pc-windows-gnu) と非互換なため使えない。代わりに `libloading` で
//! `openvr_api.dll` を動的ロードし、`VR_GetGenericInterface("FnTable:IVROverlay_028", ...)`
//! で ABI安全な関数ポインタテーブルを取得する。
//!
//! フィールドの型・順序は公式ヘッダ
//! https://github.com/ValveSoftware/openvr/blob/master/headers/openvr_capi.h
//! (取得日 2026-07-01、IVRApplicationsは2026-07-02) から逐語的に転記した。
//! 順序はABIクリティカル — 変更しないこと。
//! 実際に呼び出さないフィールドは、正しい順序・サイズを保つためのプレースホルダ
//! (`PlaceholderFn`) にしている。

use libloading::Library;
use std::ffi::{c_char, c_void};

pub const IVROVERLAY_VERSION: &str = "IVROverlay_028";
pub const IVRAPPLICATIONS_VERSION: &str = "IVRApplications_008";
/// 新しい順に試す。IVRInput_010 と 011 の違いは 011 で末尾に関数が足されたことだけで、
/// ここで呼ぶ関数(SetActionManifestPath 〜 OpenBindingUI)は同じ位置にある。
pub const IVRINPUT_VERSIONS: [&str; 2] = ["IVRInput_011", "IVRInput_010"];
/// SteamVR が終わるとき(VREvent_Quit)を知り、AcknowledgeQuit_Exiting を呼ぶためだけに使う
pub const IVRSYSTEM_VERSION: &str = "IVRSystem_026";
const VR_APPLICATION_TYPE_OVERLAY: i32 = 2;

pub type VrOverlayHandle = u64;
pub type EVRInitError = i32;
pub type EVROverlayError = i32;
pub type EVRApplicationError = i32;
pub type EVRInputError = i32;
pub const INPUT_ERROR_IPC: EVRInputError = 7;
#[allow(non_camel_case_types)]
pub type VRActionHandle_t = u64;
#[allow(non_camel_case_types)]
pub type VRActionSetHandle_t = u64;
#[allow(non_camel_case_types)]
pub type VRInputValueHandle_t = u64;
pub const INVALID_INPUT_VALUE_HANDLE: VRInputValueHandle_t = 0;

/// `VRActiveActionSet_t`(openvr.h)。nPriority は 0(通常)で使う — 0 ならゲーム(VRChat)の
/// 入力を奪わず、同じボタン押下がゲームとこのアプリの両方に届く。
#[repr(C)]
pub struct VRActiveActionSet_t {
    pub ul_action_set: VRActionSetHandle_t,
    pub ul_restricted_to_device: VRInputValueHandle_t,
    pub ul_secondary_action_set: VRActionSetHandle_t,
    pub un_padding: u32,
    pub n_priority: i32,
}

/// `InputDigitalActionData_t`(openvr.h、Windows は pack 8)
#[repr(C)]
#[derive(Default)]
pub struct InputDigitalActionData_t {
    pub b_active: bool,
    pub active_origin: VRInputValueHandle_t,
    pub b_state: bool,
    pub b_changed: bool,
    pub f_update_time: f32,
}

pub type ETextureType = i32;
pub const TEXTURE_TYPE_OPENGL: ETextureType = 1;
pub type EColorSpace = i32;
pub const COLOR_SPACE_AUTO: EColorSpace = 0;

#[repr(C)]
pub struct Texture_t {
    pub handle: *mut c_void,
    pub e_type: ETextureType,
    pub e_color_space: EColorSpace,
}

/// SteamVR が終わる。AcknowledgeQuit_Exiting → VR_Shutdown をすぐ呼ぶ(呼ばないと数秒後にプロセスごと落とされる)
pub const EVENT_QUIT: u32 = 700;
/// EVROverlayError_InvalidHandle: オーバーレイのハンドルが無効
/// (SteamVR の再起動などでサーバ側のオーバーレイが消えた)
pub const OVERLAY_ERROR_INVALID_HANDLE: EVROverlayError = 11;
/// EVROverlayError_KeyInUse: 同じキーのオーバーレイがもうある
pub const OVERLAY_ERROR_KEY_IN_USE: i32 = 17;
pub const EVENT_MOUSE_MOVE: u32 = 300;
pub const EVENT_MOUSE_BUTTON_DOWN: u32 = 301;
pub const EVENT_MOUSE_BUTTON_UP: u32 = 302;

pub type EVROverlayInputMethod = i32;
pub const INPUT_METHOD_MOUSE: EVROverlayInputMethod = 1;

/// `HmdMatrix34_t`(3行4列、行優先。最後の列が平行移動)
#[repr(C)]
pub struct HmdMatrix34_t {
    pub m: [[f32; 4]; 3],
}

/// ヘッドセット(デバイス番号 0)
pub const TRACKED_DEVICE_INDEX_HMD: u32 = 0;

#[repr(C)]
pub struct HmdVector2_t {
    pub v: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VREvent_Mouse_t {
    pub x: f32,
    pub y: f32,
    pub button: u32,
    pub cursor_index: u32,
}

/// C側の32メンバーunionのうち、Stage3で実際に使う`mouse`と、サイズ/アライメントを
/// 本物(最大メンバーである`VREvent_Reserved_t` = 6×uint64_t, 48バイト, align 8)に
/// 揃えるための`reserved`のみを転記する。
#[repr(C)]
#[derive(Clone, Copy)]
pub union VREvent_Data_t {
    pub reserved: [u64; 6],
    pub mouse: VREvent_Mouse_t,
}

#[repr(C)]
pub struct VREvent_t {
    pub event_type: u32,
    pub tracked_device_index: u32,
    pub event_age_seconds: f32,
    pub data: VREvent_Data_t,
}

/// 実際には呼び出さないフィールド用のプレースホルダ型。
/// C関数ポインタと同サイズ(8バイト/64bit)であることだけが目的。
type PlaceholderFn = Option<unsafe extern "C" fn()>;

#[repr(C)]
pub struct VrIvOverlayFnTable {
    pub find_overlay: PlaceholderFn,
    pub create_overlay:
        unsafe extern "C" fn(*mut c_char, *mut c_char, *mut VrOverlayHandle) -> EVROverlayError,
    pub create_subview_overlay: PlaceholderFn,
    pub destroy_overlay: unsafe extern "C" fn(VrOverlayHandle) -> EVROverlayError,
    pub get_overlay_key: PlaceholderFn,
    pub get_overlay_name: PlaceholderFn,
    pub set_overlay_name: PlaceholderFn,
    pub get_overlay_image_data: PlaceholderFn,
    pub get_overlay_error_name_from_enum: PlaceholderFn,
    pub set_overlay_rendering_pid: PlaceholderFn,
    pub get_overlay_rendering_pid: PlaceholderFn,
    pub set_overlay_flag: PlaceholderFn,
    pub get_overlay_flag: PlaceholderFn,
    pub get_overlay_flags: PlaceholderFn,
    pub set_overlay_color: PlaceholderFn,
    pub get_overlay_color: PlaceholderFn,
    pub set_overlay_alpha: unsafe extern "C" fn(VrOverlayHandle, f32) -> EVROverlayError,
    pub get_overlay_alpha: PlaceholderFn,
    pub set_overlay_texel_aspect: PlaceholderFn,
    pub get_overlay_texel_aspect: PlaceholderFn,
    pub set_overlay_sort_order: unsafe extern "C" fn(VrOverlayHandle, u32) -> EVROverlayError,
    pub get_overlay_sort_order: PlaceholderFn,
    pub set_overlay_width_in_meters: unsafe extern "C" fn(VrOverlayHandle, f32) -> EVROverlayError,
    pub get_overlay_width_in_meters: PlaceholderFn,
    pub set_overlay_curvature: PlaceholderFn,
    pub get_overlay_curvature: PlaceholderFn,
    pub set_overlay_pre_curve_pitch: PlaceholderFn,
    pub get_overlay_pre_curve_pitch: PlaceholderFn,
    pub set_overlay_texture_color_space: PlaceholderFn,
    pub get_overlay_texture_color_space: PlaceholderFn,
    pub set_overlay_texture_bounds: PlaceholderFn,
    pub get_overlay_texture_bounds: PlaceholderFn,
    pub get_overlay_transform_type: PlaceholderFn,
    pub set_overlay_transform_absolute: PlaceholderFn,
    pub get_overlay_transform_absolute: PlaceholderFn,
    pub set_overlay_transform_tracked_device_relative:
        unsafe extern "C" fn(VrOverlayHandle, u32, *mut HmdMatrix34_t) -> EVROverlayError,
    pub get_overlay_transform_tracked_device_relative: PlaceholderFn,
    pub set_overlay_transform_tracked_device_component: PlaceholderFn,
    pub get_overlay_transform_tracked_device_component: PlaceholderFn,
    pub set_overlay_transform_cursor: PlaceholderFn,
    pub get_overlay_transform_cursor: PlaceholderFn,
    pub set_overlay_transform_projection: PlaceholderFn,
    pub set_subview_position: PlaceholderFn,
    pub show_overlay: unsafe extern "C" fn(VrOverlayHandle) -> EVROverlayError,
    pub hide_overlay: unsafe extern "C" fn(VrOverlayHandle) -> EVROverlayError,
    pub is_overlay_visible: unsafe extern "C" fn(VrOverlayHandle) -> bool,
    pub get_transform_for_overlay_coordinates: PlaceholderFn,
    pub wait_frame_sync: unsafe extern "C" fn(u32) -> EVROverlayError,
    pub poll_next_overlay_event: unsafe extern "C" fn(VrOverlayHandle, *mut VREvent_t, u32) -> bool,
    pub get_overlay_input_method: PlaceholderFn,
    pub set_overlay_input_method:
        unsafe extern "C" fn(VrOverlayHandle, EVROverlayInputMethod) -> EVROverlayError,
    pub get_overlay_mouse_scale: PlaceholderFn,
    pub set_overlay_mouse_scale:
        unsafe extern "C" fn(VrOverlayHandle, *mut HmdVector2_t) -> EVROverlayError,
    pub compute_overlay_intersection: PlaceholderFn,
    pub is_hover_target_overlay: PlaceholderFn,
    pub set_overlay_intersection_mask: PlaceholderFn,
    pub trigger_laser_mouse_haptic_vibration:
        unsafe extern "C" fn(VrOverlayHandle, f32, f32, f32) -> EVROverlayError,
    pub set_overlay_cursor: PlaceholderFn,
    pub set_overlay_cursor_position_override: PlaceholderFn,
    pub clear_overlay_cursor_position_override: PlaceholderFn,
    pub set_overlay_texture:
        unsafe extern "C" fn(VrOverlayHandle, *mut Texture_t) -> EVROverlayError,
    pub clear_overlay_texture: PlaceholderFn,
    pub set_overlay_raw:
        unsafe extern "C" fn(VrOverlayHandle, *mut c_void, u32, u32, u32) -> EVROverlayError,
    pub set_overlay_from_file:
        unsafe extern "C" fn(VrOverlayHandle, *const c_char) -> EVROverlayError,
    pub get_overlay_texture: PlaceholderFn,
    pub release_native_overlay_handle: PlaceholderFn,
    pub get_overlay_texture_size: PlaceholderFn,
    pub create_dashboard_overlay: unsafe extern "C" fn(
        *mut c_char,
        *mut c_char,
        *mut VrOverlayHandle,
        *mut VrOverlayHandle,
    ) -> EVROverlayError,
    pub is_dashboard_visible: PlaceholderFn,
    pub is_active_dashboard_overlay: PlaceholderFn,
    pub set_dashboard_overlay_scene_process: PlaceholderFn,
    pub get_dashboard_overlay_scene_process: PlaceholderFn,
    pub show_dashboard: PlaceholderFn,
    pub get_primary_dashboard_device: PlaceholderFn,
    pub show_keyboard: PlaceholderFn,
    pub show_keyboard_for_overlay: PlaceholderFn,
    pub get_keyboard_text: PlaceholderFn,
    pub hide_keyboard: PlaceholderFn,
    pub set_keyboard_transform_absolute: PlaceholderFn,
    pub set_keyboard_position_for_overlay: PlaceholderFn,
    pub show_message_overlay: PlaceholderFn,
    pub close_message_overlay: PlaceholderFn,
}

/// `VR_IVROverlay_FnTable` の総フィールド数(公式ヘッダより逐語転記で確認した数)。
/// フィールドの追加・削除ミスを検知するトリップワイヤー。
#[cfg(test)]
const OVERLAY_FN_TABLE_FIELD_COUNT: usize = 82;

#[repr(C)]
pub struct VrIvApplicationsFnTable {
    pub add_application_manifest: unsafe extern "C" fn(*mut c_char, bool) -> EVRApplicationError,
    pub remove_application_manifest: PlaceholderFn,
    pub is_application_installed: unsafe extern "C" fn(*mut c_char) -> bool,
    pub get_application_count: PlaceholderFn,
    pub get_application_key_by_index: PlaceholderFn,
    pub get_application_key_by_process_id: PlaceholderFn,
    pub launch_application: PlaceholderFn,
    pub launch_template_application: PlaceholderFn,
    pub launch_application_from_mime_type: PlaceholderFn,
    pub launch_dashboard_overlay: PlaceholderFn,
    pub cancel_application_launch: PlaceholderFn,
    pub identify_application: unsafe extern "C" fn(u32, *mut c_char) -> EVRApplicationError,
    pub get_application_process_id: PlaceholderFn,
    pub get_applications_error_name_from_enum: PlaceholderFn,
    pub get_application_property_string: PlaceholderFn,
    pub get_application_property_bool: PlaceholderFn,
    pub get_application_property_uint64: PlaceholderFn,
    pub set_application_auto_launch: unsafe extern "C" fn(*mut c_char, bool) -> EVRApplicationError,
    pub get_application_auto_launch: unsafe extern "C" fn(*mut c_char) -> bool,
    pub set_default_application_for_mime_type: PlaceholderFn,
    pub get_default_application_for_mime_type: PlaceholderFn,
    pub get_application_supported_mime_types: PlaceholderFn,
    pub get_applications_that_support_mime_type: PlaceholderFn,
    pub get_application_launch_arguments: PlaceholderFn,
    pub get_starting_application: PlaceholderFn,
    pub get_scene_application_state: PlaceholderFn,
    pub perform_application_prelaunch_check: PlaceholderFn,
    pub get_scene_application_state_name_from_enum: PlaceholderFn,
    pub launch_internal_process: PlaceholderFn,
    pub register_subprocess: PlaceholderFn,
    pub get_current_scene_process_id: PlaceholderFn,
}

/// `VR_IVRApplications_FnTable` の総フィールド数(公式ヘッダより逐語転記で確認した数)。
#[cfg(test)]
const APPLICATIONS_FN_TABLE_FIELD_COUNT: usize = 31;

/// `VR_IVRInput_FnTable`(openvr_capi.h、取得日 2026-10-18、IVRInput_011)。
/// 呼ぶのは先頭6つだけ。残りは並びとサイズを保つためのプレースホルダ。
#[repr(C)]
pub struct VrIvInputFnTable {
    pub set_action_manifest_path: unsafe extern "C" fn(*mut c_char) -> EVRInputError,
    pub get_action_set_handle:
        unsafe extern "C" fn(*mut c_char, *mut VRActionSetHandle_t) -> EVRInputError,
    pub get_action_handle:
        unsafe extern "C" fn(*mut c_char, *mut VRActionHandle_t) -> EVRInputError,
    pub get_input_source_handle: PlaceholderFn,
    pub update_action_state:
        unsafe extern "C" fn(*mut VRActiveActionSet_t, u32, u32) -> EVRInputError,
    pub get_digital_action_data: unsafe extern "C" fn(
        VRActionHandle_t,
        *mut InputDigitalActionData_t,
        u32,
        VRInputValueHandle_t,
    ) -> EVRInputError,
    pub get_analog_action_data: PlaceholderFn,
    pub get_pose_action_data_relative_to_now: PlaceholderFn,
    pub get_pose_action_data_for_next_frame: PlaceholderFn,
    pub get_skeletal_action_data: PlaceholderFn,
    pub get_dominant_hand: PlaceholderFn,
    pub set_dominant_hand: PlaceholderFn,
    pub get_eye_tracking_data_relative_to_now: PlaceholderFn,
    pub get_eye_tracking_data_for_next_frame: PlaceholderFn,
    pub get_bone_count: PlaceholderFn,
    pub get_bone_hierarchy: PlaceholderFn,
    pub get_bone_name: PlaceholderFn,
    pub get_skeletal_reference_transforms: PlaceholderFn,
    pub get_skeletal_tracking_level: PlaceholderFn,
    pub get_skeletal_bone_data: PlaceholderFn,
    pub get_skeletal_summary_data: PlaceholderFn,
    pub get_skeletal_bone_data_compressed: PlaceholderFn,
    pub decompress_skeletal_bone_data: PlaceholderFn,
    pub trigger_haptic_vibration_action: PlaceholderFn,
    pub get_action_origins: PlaceholderFn,
    pub get_origin_localized_name: PlaceholderFn,
    pub get_origin_tracked_device_info: PlaceholderFn,
    pub get_action_binding_info: PlaceholderFn,
    pub show_action_origins: PlaceholderFn,
    pub show_bindings_for_action_set: PlaceholderFn,
    pub get_component_state_for_binding: PlaceholderFn,
    pub is_using_legacy_input: PlaceholderFn,
    pub open_binding_ui: unsafe extern "C" fn(
        *mut c_char,
        VRActionSetHandle_t,
        VRInputValueHandle_t,
        bool,
    ) -> EVRInputError,
    pub get_binding_variant: PlaceholderFn,
}

#[cfg(test)]
const INPUT_FN_TABLE_FIELD_COUNT: usize = 34;

/// openvr_api.dll をロードし、IVROverlay の関数ポインタテーブルを保持する。
/// `_lib` が生きている間だけ `overlay_table` は有効。
/// `VR_IVRSystem_FnTable`(openvr_capi.h、取得日 2026-10-18、IVRSystem_026)。
/// 呼ぶのは PollNextEvent と AcknowledgeQuit_Exiting だけ。残りは並びとサイズを保つためのプレースホルダ。
#[repr(C)]
pub struct VrIvSystemFnTable {
    pub get_recommended_render_target_size: PlaceholderFn,
    pub get_projection_matrix: PlaceholderFn,
    pub get_projection_raw: PlaceholderFn,
    pub compute_distortion: PlaceholderFn,
    pub compute_distortion_set: PlaceholderFn,
    pub get_eye_to_head_transform: PlaceholderFn,
    pub get_time_since_last_vsync: PlaceholderFn,
    pub get_d3d9_adapter_index: PlaceholderFn,
    pub get_dxgi_output_info: PlaceholderFn,
    pub get_output_device: PlaceholderFn,
    pub is_display_on_desktop: PlaceholderFn,
    pub set_display_visibility: PlaceholderFn,
    pub get_device_to_absolute_tracking_pose: PlaceholderFn,
    pub get_seated_zero_pose_to_standing_absolute_tracking_pose: PlaceholderFn,
    pub get_raw_zero_pose_to_standing_absolute_tracking_pose: PlaceholderFn,
    pub get_sorted_tracked_device_indices_of_class: PlaceholderFn,
    pub get_tracked_device_activity_level: PlaceholderFn,
    pub apply_transform: PlaceholderFn,
    pub get_tracked_device_index_for_controller_role: PlaceholderFn,
    pub get_controller_role_for_tracked_device_index: PlaceholderFn,
    pub get_tracked_device_class: PlaceholderFn,
    pub is_tracked_device_connected: PlaceholderFn,
    pub get_bool_tracked_device_property: PlaceholderFn,
    pub get_float_tracked_device_property: PlaceholderFn,
    pub get_int32_tracked_device_property: PlaceholderFn,
    pub get_uint64_tracked_device_property: PlaceholderFn,
    pub get_matrix34_tracked_device_property: PlaceholderFn,
    pub get_array_tracked_device_property: PlaceholderFn,
    pub get_string_tracked_device_property: PlaceholderFn,
    pub get_prop_error_name_from_enum: PlaceholderFn,
    pub poll_next_event: unsafe extern "C" fn(*mut VREvent_t, u32) -> bool,
    pub poll_next_event_with_pose: PlaceholderFn,
    pub poll_next_event_with_pose_and_overlays: PlaceholderFn,
    pub get_event_type_name_from_enum: PlaceholderFn,
    pub get_hidden_area_mesh: PlaceholderFn,
    pub get_eye_tracked_foveation_center: PlaceholderFn,
    pub get_eye_tracked_foveation_center_for_projection: PlaceholderFn,
    pub get_controller_state: PlaceholderFn,
    pub get_controller_state_with_pose: PlaceholderFn,
    pub trigger_haptic_pulse: PlaceholderFn,
    pub get_button_id_name_from_enum: PlaceholderFn,
    pub get_controller_axis_type_name_from_enum: PlaceholderFn,
    pub is_input_available: PlaceholderFn,
    pub is_steam_vrdrawing_controllers: PlaceholderFn,
    pub should_application_pause: PlaceholderFn,
    pub should_application_reduce_rendering_work: PlaceholderFn,
    pub perform_firmware_update: PlaceholderFn,
    pub acknowledge_quit_exiting: unsafe extern "C" fn(),
    pub get_app_container_file_paths: PlaceholderFn,
    pub get_runtime_version: PlaceholderFn,
    pub set_sdkversion: PlaceholderFn,
}

/// `VR_IVRSystem_FnTable` の総フィールド数(公式ヘッダより)
#[cfg(test)]
const SYSTEM_FN_TABLE_FIELD_COUNT: usize = 51;

pub struct OpenVrLibrary {
    _lib: Library,
    overlay_table: *const VrIvOverlayFnTable,
    applications_table: Option<*const VrIvApplicationsFnTable>,
    input_table: Option<*const VrIvInputFnTable>,
    system_table: Option<*const VrIvSystemFnTable>,
}

impl OpenVrLibrary {
    pub fn load() -> Result<Self, String> {
        let lib = unsafe { Library::new("openvr_api.dll") }
            .map_err(|e| format!("DLLロード失敗: {}", e))?;

        let vr_init_internal: libloading::Symbol<
            unsafe extern "C" fn(*mut EVRInitError, i32) -> isize,
        > = unsafe { lib.get(b"VR_InitInternal\0") }
            .map_err(|e| format!("VR_InitInternal が見つからない: {}", e))?;

        let mut init_error: EVRInitError = 0;
        let token = unsafe { vr_init_internal(&mut init_error, VR_APPLICATION_TYPE_OVERLAY) };
        if init_error != 0 || token == 0 {
            return Err(format!("VR_InitInternal エラー: code={}", init_error));
        }
        // ここから先で失敗したら、つないだままにせず VR_Shutdown してから返す
        let shutdown_on_error = |e: String| -> String {
            if let Ok(f) = unsafe { lib.get::<unsafe extern "C" fn()>(b"VR_ShutdownInternal\0") } {
                unsafe { f() };
            }
            e
        };

        let vr_get_generic_interface: libloading::Symbol<
            unsafe extern "C" fn(*const c_char, *mut EVRInitError) -> isize,
        > = unsafe { lib.get(b"VR_GetGenericInterface\0") }.map_err(|e| {
            shutdown_on_error(format!("VR_GetGenericInterface が見つからない: {}", e))
        })?;

        let version = std::ffi::CString::new(format!("FnTable:{}", IVROVERLAY_VERSION)).unwrap();
        let mut iface_error: EVRInitError = 0;
        let table_ptr = unsafe { vr_get_generic_interface(version.as_ptr(), &mut iface_error) };
        if iface_error != 0 || table_ptr == 0 {
            return Err(shutdown_on_error(format!(
                "IVROverlay インターフェース取得失敗: code={}",
                iface_error
            )));
        }

        // IVRApplications は自動起動登録(SteamVR起動時にこのexeを立ち上げてもらう)のためだけに
        // 使う補助インターフェース。取得できなくてもオーバーレイ本体には影響させない(non-fatal)。
        let applications_version =
            std::ffi::CString::new(format!("FnTable:{}", IVRAPPLICATIONS_VERSION)).unwrap();
        let mut applications_iface_error: EVRInitError = 0;
        let applications_table_ptr = unsafe {
            vr_get_generic_interface(applications_version.as_ptr(), &mut applications_iface_error)
        };
        let applications_table = if applications_iface_error != 0 || applications_table_ptr == 0 {
            crate::log!(
                "[SteamVR] IVRApplications インターフェース取得失敗 (自動起動登録は無効): code={}",
                applications_iface_error
            );
            None
        } else {
            Some(applications_table_ptr as *const VrIvApplicationsFnTable)
        };

        // IVRInput はボタン操作(SteamVR のバインド)を受け取るためだけに使う。
        // 取れなくてもオーバーレイ本体には影響させない(non-fatal)。
        let mut input_table = None;
        for v in IVRINPUT_VERSIONS {
            let name = std::ffi::CString::new(format!("FnTable:{}", v)).unwrap();
            let mut err: EVRInitError = 0;
            let ptr = unsafe { vr_get_generic_interface(name.as_ptr(), &mut err) };
            if err == 0 && ptr != 0 {
                input_table = Some(ptr as *const VrIvInputFnTable);
                break;
            }
            crate::log!("[SteamVR] {} インターフェース取得失敗: code={}", v, err);
        }

        // IVRSystem は SteamVR の終了(VREvent_Quit)を知るためだけに使う。取れなくても
        // vrserver.exe の有無の見張りで気づけるので non-fatal
        let system_name = std::ffi::CString::new(format!("FnTable:{}", IVRSYSTEM_VERSION)).unwrap();
        let mut system_err: EVRInitError = 0;
        let system_ptr = unsafe { vr_get_generic_interface(system_name.as_ptr(), &mut system_err) };
        let system_table = if system_err == 0 && system_ptr != 0 {
            Some(system_ptr as *const VrIvSystemFnTable)
        } else {
            crate::log!(
                "[SteamVR] {} インターフェース取得失敗: code={}",
                IVRSYSTEM_VERSION,
                system_err
            );
            None
        };

        Ok(Self {
            _lib: lib,
            overlay_table: table_ptr as *const VrIvOverlayFnTable,
            applications_table,
            input_table,
            system_table,
        })
    }

    pub fn overlay(&self) -> &VrIvOverlayFnTable {
        unsafe { &*self.overlay_table }
    }

    pub fn applications(&self) -> Option<&VrIvApplicationsFnTable> {
        self.applications_table.map(|ptr| unsafe { &*ptr })
    }

    pub fn input(&self) -> Option<&VrIvInputFnTable> {
        self.input_table.map(|ptr| unsafe { &*ptr })
    }

    pub fn system(&self) -> Option<&VrIvSystemFnTable> {
        self.system_table.map(|ptr| unsafe { &*ptr })
    }
}

impl Drop for OpenVrLibrary {
    fn drop(&mut self) {
        let shutdown: Result<libloading::Symbol<unsafe extern "C" fn()>, _> =
            unsafe { self._lib.get(b"VR_ShutdownInternal\0") };
        if let Ok(vr_shutdown_internal) = shutdown {
            unsafe { vr_shutdown_internal() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fn_table_field_count_matches_official_header() {
        assert_eq!(
            std::mem::size_of::<VrIvOverlayFnTable>(),
            OVERLAY_FN_TABLE_FIELD_COUNT * std::mem::size_of::<usize>(),
            "VR_IVROverlay_FnTable のフィールド数が公式ヘッダと食い違っている可能性がある"
        );
    }

    #[test]
    fn test_applications_fn_table_field_count_matches_official_header() {
        assert_eq!(
            std::mem::size_of::<VrIvApplicationsFnTable>(),
            APPLICATIONS_FN_TABLE_FIELD_COUNT * std::mem::size_of::<usize>(),
            "VR_IVRApplications_FnTable のフィールド数が公式ヘッダと食い違っている可能性がある"
        );
    }

    #[test]
    fn test_system_fn_table_field_count_matches_official_header() {
        assert_eq!(
            std::mem::size_of::<VrIvSystemFnTable>(),
            SYSTEM_FN_TABLE_FIELD_COUNT * std::mem::size_of::<usize>(),
            "VR_IVRSystem_FnTable のフィールド数が公式ヘッダと食い違っている可能性がある"
        );
        // PollNextEvent は 31 番目、AcknowledgeQuit_Exiting は 48 番目(0 始まりで 30 と 47)
        assert_eq!(
            std::mem::offset_of!(VrIvSystemFnTable, poll_next_event),
            30 * std::mem::size_of::<usize>()
        );
        assert_eq!(
            std::mem::offset_of!(VrIvSystemFnTable, acknowledge_quit_exiting),
            47 * std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn test_input_fn_table_field_count_matches_official_header() {
        assert_eq!(
            std::mem::size_of::<VrIvInputFnTable>(),
            INPUT_FN_TABLE_FIELD_COUNT * std::mem::size_of::<usize>(),
            "VR_IVRInput_FnTable のフィールド数が公式ヘッダと食い違っている可能性がある"
        );
    }

    #[test]
    fn test_input_struct_sizes_match_official_header() {
        // VRActiveActionSet_t: u64×3 + u32 + i32 = 32
        assert_eq!(std::mem::size_of::<VRActiveActionSet_t>(), 32);
        // InputDigitalActionData_t: bool(+7) + u64 + bool + bool(+2) + f32 = 24
        assert_eq!(std::mem::size_of::<InputDigitalActionData_t>(), 24);
        assert_eq!(std::mem::size_of::<HmdMatrix34_t>(), 48);
    }

    #[test]
    fn test_texture_t_size_matches_official_header() {
        assert_eq!(
            std::mem::size_of::<Texture_t>(),
            16,
            "Texture_t のサイズが公式ヘッダ(16バイト)と食い違っている可能性がある"
        );
    }

    #[test]
    fn test_vrevent_t_size_matches_official_header() {
        assert_eq!(
            std::mem::size_of::<VREvent_t>(),
            64,
            "VREvent_t のサイズが公式ヘッダ(64バイト)と食い違っている可能性がある"
        );
    }

    #[test]
    fn test_hmd_vector2_t_size_matches_official_header() {
        assert_eq!(
            std::mem::size_of::<HmdVector2_t>(),
            8,
            "HmdVector2_t のサイズが公式ヘッダ(8バイト)と食い違っている可能性がある"
        );
    }
}
