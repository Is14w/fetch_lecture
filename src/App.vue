<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

type Row = Record<string, unknown>;
type CaptchaReply = { imageDataUrl: string; recognizedCode: string; recognitionNote?: string | null };
type BookingReply = { message: string; success: boolean; verified: boolean };
type BookingLog = { level: string; message: string; at: string };

function createFingerprint() {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}
function getDeviceFingerprint() {
  const key = "fetch-lecture.device-fingerprint";
  try {
    const saved = window.localStorage.getItem(key);
    if (saved && /^[a-f0-9]{32}$/.test(saved)) return saved;
    const value = createFingerprint();
    window.localStorage.setItem(key, value);
    return value;
  } catch {
    return createFingerprint();
  }
}
const fingerprint = ref(getDeviceFingerprint());
const username = ref("");
const password = ref("");
const phoneCode = ref("");
const phoneStep = ref(false);
const loggedIn = ref(false);
const busy = ref(false);
const loading = ref(false);
const error = ref("");
const notice = ref("");
const tab = ref<"lectures" | "bookings">("lectures");
const lectures = ref<Row[]>([]);
const bookings = ref<Row[]>([]);
const selectedWid = ref("");
const search = ref("");
const availableOnly = ref(false);
const captcha = ref<CaptchaReply | null>(null);
const captchaCode = ref("");
const captchaBusy = ref(false);
const bookingBusy = ref(false);
const scheduledAt = ref("");
const delaySeconds = ref("0");
const reloginBeforeSeconds = ref("10");
const reauthRequired = ref(false);
const running = ref(false);
const stopping = ref(false);
const logs = ref<BookingLog[]>([]);
let unlisten: UnlistenFn | undefined;
let unlistenReauth: UnlistenFn | undefined;
let refreshTimer: number | undefined;

const selected = computed(() => lectures.value.find((row) => String(row.WID ?? row.HD_WID ?? "") === selectedWid.value) ?? null);
const availableCount = computed(() => lectures.value.filter((row) => {
  const total = Number(row.HDZRS ?? 0);
  return !total || Number(row.YYRS ?? 0) < total;
}).length);
const rows = computed(() => {
  if (tab.value === "bookings") return bookings.value;
  const query = search.value.trim().toLowerCase();
  return lectures.value.filter((row) => {
    const name = String(row.JZMC ?? "").toLowerCase();
    const total = Number(row.HDZRS ?? 0);
    const hasSeats = !total || Number(row.YYRS ?? 0) < total;
    return (!query || name.includes(query)) && (!availableOnly.value || hasSeats);
  });
});
function value(v: unknown, fallback = "—") {
  return v === undefined || v === null || v === "" ? fallback : String(v);
}
function time(v: unknown) {
  return v ? String(v).replace("T", " ").replace(/:\d{2}$/, "") : "时间待定";
}
function seats(row: Row) {
  const total = Number(row.HDZRS ?? 0);
  const reserved = Number(row.YYRS ?? 0);
  return total ? String(reserved) + " / " + String(total) : "名额暂缺";
}
function localDateTime(v: unknown) {
  const parsed = new Date(String(v ?? "").replace(" ", "T"));
  if (Number.isNaN(parsed.getTime())) return "";
  return new Date(parsed.getTime() - parsed.getTimezoneOffset() * 60000).toISOString().slice(0, 16);
}
function addLog(entry: BookingLog) {
  logs.value.unshift(entry);
}
async function clearLogs() {
  try {
    await invoke("clear_booking_logs");
    logs.value = [];
  } catch (reason) {
    error.value = String(reason);
  }
}
async function openBookingLog() {
  try { await invoke("open_booking_log"); }
  catch (reason) { error.value = String(reason); }
}
async function loadData() {
  loading.value = true;
  error.value = "";
  try {
    const [lectureRows, bookingRows] = await Promise.all([
      invoke<Row[]>("list_lectures"),
      invoke<Row[]>("list_my_bookings"),
    ]);
    lectures.value = lectureRows;
    bookings.value = bookingRows;
    if (!lectures.value.some((row) => String(row.WID ?? row.HD_WID ?? "") === selectedWid.value)) {
      selectedWid.value = String(lectures.value[0]?.WID ?? lectures.value[0]?.HD_WID ?? "");
    }
    if (selected.value && !scheduledAt.value) scheduledAt.value = localDateTime(selected.value.YYKSSJ);
  } catch (reason) {
    error.value = String(reason);
  } finally {
    loading.value = false;
  }
}
async function login() {
  if (!username.value.trim() || !password.value) { error.value = "请输入学号和密码"; return; }
  busy.value = true; error.value = ""; notice.value = "";
  try {
    const result = await invoke<{ requiresPhoneCode: boolean; message: string | null }>("login", {
      username: username.value.trim(), password: password.value, fingerprint: fingerprint.value,
    });
    if (result.requiresPhoneCode) {
      phoneStep.value = true;
      notice.value = result.message || "此设备需要短信验证，请发送验证码";
    } else {
      await finishLogin();
    }
  } catch (reason) { error.value = String(reason); }
  finally { busy.value = false; }
}
async function sendSms() {
  busy.value = true; error.value = "";
  try { notice.value = await invoke<string>("send_phone_code") || "短信验证码已发送"; }
  catch (reason) { error.value = String(reason); }
  finally { busy.value = false; }
}
async function verifyPhone() {
  if (!phoneCode.value.trim()) { error.value = "请输入短信验证码"; return; }
  busy.value = true; error.value = "";
  try {
    await invoke("complete_phone_login", { phoneCode: phoneCode.value.trim() });
    if (reauthRequired.value) {
      reauthRequired.value = false; phoneCode.value = "";
      notice.value = "登录会话已刷新，自动预约将继续";
      await loadData();
    } else {
      await finishLogin();
    }
  } catch (reason) { error.value = String(reason); }
  finally { busy.value = false; }
}
async function finishLogin() {
  loggedIn.value = true; password.value = ""; phoneCode.value = ""; phoneStep.value = false;
  notice.value = "已安全登录，会话仅在当前运行期间保留";
  await loadData();
}
async function logout() {
  try { await invoke("logout"); } catch { /* Clear the view if the app is closing. */ }
  loggedIn.value = false; phoneStep.value = false; lectures.value = []; bookings.value = [];
  selectedWid.value = ""; captcha.value = null; captchaCode.value = ""; reauthRequired.value = false;
}
async function refreshCaptcha() {
  if (!selectedWid.value) return;
  captchaBusy.value = true; error.value = "";
  try {
    captcha.value = await invoke<CaptchaReply>("get_captcha");
    captchaCode.value = captcha.value.recognizedCode;
    if (captcha.value.recognitionNote) notice.value = "OCR 暂不可用，可手动输入验证码：" + captcha.value.recognitionNote;
  } catch (reason) { error.value = String(reason); }
  finally { captchaBusy.value = false; }
}
async function choose(row: Row) {
  selectedWid.value = String(row.WID ?? row.HD_WID ?? "");
  scheduledAt.value = localDateTime(row.YYKSSJ);
  captcha.value = null; captchaCode.value = "";
  await refreshCaptcha();
}
async function reserveOnce() {
  if (!selectedWid.value || !captchaCode.value.trim()) { error.value = "请先获取验证码，并确认识别结果"; return; }
  bookingBusy.value = true; error.value = "";
  try {
    const result = await invoke<BookingReply>("reserve_lecture", {
      lectureWid: selectedWid.value, captchaCode: captchaCode.value.trim(),
    });
    notice.value = result.message;
    if (result.success) { captcha.value = null; await loadData(); }
    else await refreshCaptcha();
  } catch (reason) { error.value = String(reason); }
  finally { bookingBusy.value = false; }
}
async function startBooking(at: number) {
  if (!selectedWid.value || running.value) return;
  const delay = Number(delaySeconds.value);
  const reloginBefore = Number(reloginBeforeSeconds.value);
  if (!Number.isFinite(delay) || delay < -60 || delay > 60) { error.value = "微调秒数需在 -60 到 60 之间"; return; }
  if (!Number.isFinite(reloginBefore) || reloginBefore < 0 || reloginBefore > 300) { error.value = "提前登录时间需在 0 到 300 秒之间"; return; }
  error.value = "";
  running.value = true;
  stopping.value = false;
  try {
    await invoke("start_booking", { lectureWid: selectedWid.value, startAtMs: at, delaySeconds: delay, reloginBeforeSeconds: reloginBefore });
  } catch (reason) {
    running.value = false;
    stopping.value = false;
    error.value = String(reason);
  }
}
async function stopBooking() {
  if (stopping.value) return;
  stopping.value = true;
  try {
    const stopped = await invoke<boolean>("stop_booking");
    if (!stopped) {
      running.value = false;
      stopping.value = false;
      reauthRequired.value = false;
    }
  } catch (reason) {
    error.value = String(reason);
    stopping.value = false;
  }
}
onMounted(async () => {
  try {
    unlisten = await listen<BookingLog>("booking-log", ({ payload }) => {
      addLog(payload);
      if (["stopped", "success", "error"].includes(payload.level)) {
        reauthRequired.value = false;
        running.value = false;
        stopping.value = false;
        if (payload.level === "success") void loadData();
      }
    });
  } catch { /* The event bridge is available inside the Tauri desktop shell. */ }
  try {
    const saved = await invoke<BookingLog[]>("load_booking_logs");
    const currentKeys = new Set(logs.value.map((log) => `${log.at}\0${log.level}\0${log.message}`));
    logs.value = [
      ...logs.value,
      ...saved.filter((log) => !currentKeys.has(`${log.at}\0${log.level}\0${log.message}`)),
    ];
  } catch (reason) {
    error.value = String(reason);
  }
  try {
    unlistenReauth = await listen<{ message: string }>("reauth-required", ({ payload }) => {
      reauthRequired.value = true;
      phoneCode.value = "";
      error.value = "";
      notice.value = payload.message;
    });
  } catch { /* The event bridge is available inside the Tauri desktop shell. */ }
  refreshTimer = window.setInterval(() => {
    if (loggedIn.value && tab.value === "bookings") void loadData();
  }, 30000);
});
onUnmounted(() => {
  unlisten?.();
  unlistenReauth?.();
  if (refreshTimer) window.clearInterval(refreshTimer);
});
</script>

<template>
  <div class="min-h-screen bg-canvas text-ink">
    <header class="border-b border-gray-200 bg-white">
      <div class="mx-auto flex max-w-7xl items-center justify-between px-6 py-4">
        <div class="flex items-center gap-3">
          <div class="flex h-10 w-10 items-center justify-center rounded-xl bg-brand text-lg font-bold text-white">F</div>
          <div><div class="text-sm font-bold">Fetch Lecture</div><div class="text-xs text-muted">东南大学 · 研究生素质讲座</div></div>
        </div>
        <div v-if="loggedIn" class="flex items-center gap-3">
          <span class="rounded-full bg-green-50 px-3 py-1.5 text-xs font-semibold text-green-700">● 会话已连接</span>
          <span class="text-sm text-muted">{{ username }}</span>
          <button class="soft-button" @click="logout">退出</button>
        </div>
        <div v-else class="rounded-full bg-gray-100 px-3 py-1.5 text-xs font-semibold text-muted">安全登录 · 本地处理</div>
      </div>
    </header>

    <main v-if="!loggedIn" class="mx-auto grid min-h-screen max-w-6xl items-center gap-10 px-6 py-12 lg:grid-cols-2">
      <section class="max-w-xl">
        <div class="mb-5 inline-flex items-center gap-2 rounded-full border border-blue-100 bg-white px-3 py-1.5 text-xs font-semibold text-brand shadow-soft">
          <span class="h-2 w-2 rounded-full bg-green-400"></span>桌面端重构
        </div>
        <h1 class="max-w-lg text-4xl font-bold leading-tight sm:text-5xl">把讲座预约，<span class="text-brand">安排得更从容。</span></h1>
        <p class="mt-5 max-w-lg text-base leading-7 text-muted">查看讲座与预约名额，提前设定开抢时间。验证码在本机由 Rust + ONNX 自动识别，密码不会写入磁盘；设备标识保存在本机，用于复用学校的可信设备记录。</p>
        <div class="mt-8 grid gap-3 sm:grid-cols-3">
          <div class="rounded-2xl bg-white p-4 shadow-soft"><div class="mb-3 text-sm font-bold text-brand">01</div><div class="text-sm font-semibold">登录认证</div><div class="mt-1 text-xs text-muted">支持短信二次验证</div></div>
          <div class="rounded-2xl bg-white p-4 shadow-soft"><div class="mb-3 text-sm font-bold text-green-700">02</div><div class="text-sm font-semibold">本地 OCR</div><div class="mt-1 text-xs text-muted">ONNX 离线推理</div></div>
          <div class="rounded-2xl bg-white p-4 shadow-soft"><div class="mb-3 text-sm font-bold text-yellow-700">03</div><div class="text-sm font-semibold">定时预约</div><div class="mt-1 text-xs text-muted">状态实时记录</div></div>
        </div>
      </section>
      <section class="mx-auto w-full max-w-md rounded-3xl border border-gray-100 bg-white p-7 shadow-card sm:p-9">
        <div class="mb-7">
          <div class="text-xs font-bold uppercase tracking-widest text-brand">Welcome back</div>
          <h2 class="mt-2 text-2xl font-bold">{{ phoneStep ? "完成安全验证" : "登录统一身份认证" }}</h2>
          <p class="mt-2 text-sm leading-6 text-muted">{{ phoneStep ? "当前设备需要短信验证码确认身份。" : "使用东南大学统一身份认证账号继续。" }}</p>
        </div>
        <form v-if="!phoneStep" class="space-y-4" @submit.prevent="login">
          <label class="block"><span class="mb-2 block text-sm font-semibold">学号</span><input v-model="username" class="field" autocomplete="username" placeholder="请输入学号" /></label>
          <label class="block"><span class="mb-2 block text-sm font-semibold">密码</span><input v-model="password" class="field" type="password" autocomplete="current-password" placeholder="请输入统一身份认证密码" /></label>
          <button class="primary-button mt-2 w-full" type="submit" :disabled="busy">{{ busy ? "正在连接…" : "登录并加载讲座" }}</button>
        </form>
        <form v-else class="space-y-4" @submit.prevent="verifyPhone">
          <label class="block"><span class="mb-2 block text-sm font-semibold">短信验证码</span><input v-model="phoneCode" class="field tracking-widest" inputmode="numeric" autocomplete="one-time-code" placeholder="输入收到的验证码" /></label>
          <button class="primary-button w-full" type="submit" :disabled="busy">{{ busy ? "正在验证…" : "验证并登录" }}</button>
          <button class="soft-button w-full" type="button" :disabled="busy" @click="sendSms">发送短信验证码</button>
          <button class="w-full py-2 text-sm font-semibold text-muted" type="button" @click="phoneStep = false; error = ''">返回账号登录</button>
        </form>
        <p v-if="error" class="mt-4 rounded-xl bg-red-50 px-4 py-3 text-sm text-red-700">{{ error }}</p>
        <p v-else-if="notice" class="mt-4 rounded-xl bg-blue-50 px-4 py-3 text-sm text-blue-800">{{ notice }}</p>
        <div class="mt-7 border-t border-gray-100 pt-5 text-xs leading-5 text-muted">认证信息仅用于向学校服务登录；密码不会保存到磁盘，设备标识保存在本机以复用可信设备记录。</div>
      </section>
    </main>

    <main v-else class="mx-auto max-w-7xl px-5 py-7 sm:px-8">
      <div class="mb-7 flex flex-col justify-between gap-4 md:flex-row md:items-end">
        <div><div class="text-sm text-muted">讲座预约工作台</div><h1 class="mt-1 text-3xl font-bold">你好，{{ username }}</h1></div>
        <div class="flex flex-wrap gap-2">
          <button class="soft-button" :disabled="loading" @click="loadData">{{ loading ? "刷新中…" : "↻ 刷新数据" }}</button>
          <button class="primary-button" :disabled="!selectedWid || running" @click="startBooking(Date.now())">立即自动预约</button>
          <button v-if="running" class="soft-button text-red-600" @click="stopBooking">停止</button>
        </div>
      </div>
      <div v-if="error" class="mb-5 rounded-2xl border border-red-100 bg-red-50 px-5 py-4 text-sm text-red-700">{{ error }}</div>
      <div v-else-if="notice" class="mb-5 rounded-2xl border border-blue-100 bg-blue-50 px-5 py-4 text-sm text-blue-800">{{ notice }}</div>

      <section class="mb-6 grid gap-4 sm:grid-cols-3">
        <div class="rounded-2xl bg-white p-5 shadow-soft"><div class="text-sm text-muted">可预约讲座</div><div class="mt-3 text-3xl font-bold">{{ lectures.length }}</div><div class="mt-1 text-xs text-muted">{{ availableCount }} 场仍有名额</div></div>
        <div class="rounded-2xl bg-white p-5 shadow-soft"><div class="text-sm text-muted">我的预约</div><div class="mt-3 text-3xl font-bold">{{ bookings.length }}</div><div class="mt-1 text-xs text-muted">预约状态来自学校系统</div></div>
        <div class="rounded-2xl bg-white p-5 shadow-soft"><div class="text-sm text-muted">自动预约</div><div class="mt-3 text-lg font-bold">{{ running ? "正在等待 / 重试" : "当前待命" }}</div><div class="mt-1 text-xs text-muted">验证码由 Rust + ONNX 识别</div></div>
      </section>

      <section class="grid gap-5 xl:grid-cols-2">
        <div class="overflow-hidden rounded-2xl bg-white shadow-soft">
          <div class="flex flex-col justify-between gap-3 border-b border-gray-100 px-5 py-4 sm:flex-row sm:items-center">
            <div><h2 class="font-bold">讲座列表</h2><p class="mt-1 text-xs text-muted">选择一场讲座查看时间与名额</p></div>
            <div class="flex gap-2"><button class="rounded-lg px-3 py-2 text-sm font-semibold" :class="tab === 'lectures' ? 'bg-blue-50 text-brand' : 'text-muted'" @click="tab = 'lectures'">可预约</button><button class="rounded-lg px-3 py-2 text-sm font-semibold" :class="tab === 'bookings' ? 'bg-blue-50 text-brand' : 'text-muted'" @click="tab = 'bookings'">我的预约</button></div>
          </div>
          <div class="flex gap-3 border-b border-gray-100 px-5 py-3">
            <input v-model="search" class="field py-2" placeholder="搜索讲座名称" />
            <label class="inline-flex shrink-0 items-center gap-2 rounded-xl bg-gray-50 px-3 text-xs text-muted"><input v-model="availableOnly" type="checkbox" />有名额</label>
          </div>
          <div class="max-h-96 overflow-auto">
            <div v-if="loading" class="p-10 text-center text-sm text-muted">正在加载讲座…</div>
            <div v-else-if="!rows.length" class="p-10 text-center text-sm text-muted">暂时没有列表数据，请刷新</div>
            <button v-for="row in rows" :key="String(row.WID ?? row.HD_WID ?? row.JZMC)" class="flex w-full items-start gap-4 border-b border-gray-50 px-5 py-4 text-left hover:bg-blue-50" :class="String(row.WID ?? row.HD_WID ?? '') === selectedWid && tab === 'lectures' ? 'bg-blue-50' : ''" @click="tab === 'lectures' ? choose(row) : undefined">
              <span class="mt-1 flex h-9 w-9 shrink-0 items-center justify-center rounded-xl bg-gray-50 text-sm text-brand">{{ tab === "lectures" ? "讲" : "✓" }}</span>
              <span class="min-w-0 flex-1"><span class="block truncate text-sm font-semibold">{{ value(row.JZMC, "未命名讲座") }}</span><span class="mt-1 block text-xs text-muted">预约 {{ time(row.YYKSSJ) }} — {{ time(row.YYJSSJ) }}</span><span class="mt-1 block text-xs text-muted">活动 {{ time(row.JZSJ) }}</span></span>
              <span class="shrink-0 text-right"><span class="block text-xs font-semibold text-green-700">{{ seats(row) }}</span><span class="mt-1 block text-xs text-muted">已预约 / 总名额</span></span>
            </button>
          </div>
        </div>

        <aside class="space-y-5">
          <div class="rounded-2xl bg-white p-5 shadow-soft">
            <div class="flex items-start justify-between gap-3"><div><div class="text-xs font-bold uppercase tracking-widest text-brand">Selected lecture</div><h2 class="mt-2 text-lg font-bold">{{ value(selected?.JZMC, "先从列表选择讲座") }}</h2></div><span class="rounded-xl bg-blue-50 px-3 py-2 text-xs font-semibold text-brand">{{ selected ? seats(selected) : "—" }}</span></div>
            <div v-if="selected" class="mt-4 space-y-3 border-t border-gray-100 pt-4 text-sm">
              <div class="flex justify-between gap-3"><span class="text-muted">预约时间</span><span>{{ time(selected.YYKSSJ) }} – {{ time(selected.YYJSSJ) }}</span></div>
              <div class="flex justify-between gap-3"><span class="text-muted">活动时间</span><span>{{ time(selected.JZSJ) }}</span></div>
              <div class="flex justify-between gap-3"><span class="text-muted">预约编号</span><span class="truncate font-mono text-xs">{{ value(selected.WID ?? selected.HD_WID) }}</span></div>
            </div>
          </div>

          <div class="rounded-2xl bg-white p-5 shadow-soft">
            <div class="flex items-center justify-between"><div><h2 class="font-bold">验证码识别</h2><p class="mt-1 text-xs text-muted">本机 ONNX 自动识别，可手动校正</p></div><button class="soft-button" :disabled="captchaBusy || !selectedWid" @click="refreshCaptcha">{{ captchaBusy ? "识别中…" : "刷新" }}</button></div>
            <div class="mt-4 flex items-center gap-4 rounded-xl bg-gray-50 p-3">
              <div class="flex h-14 w-28 shrink-0 items-center justify-center overflow-hidden rounded-lg bg-white"><img v-if="captcha?.imageDataUrl" :src="captcha.imageDataUrl" alt="预约验证码" class="h-full w-full object-contain" /><span v-else class="text-xs text-muted">{{ captchaBusy ? "加载中" : "等待验证码" }}</span></div>
              <label class="min-w-0 flex-1"><span class="mb-1 block text-xs font-semibold text-muted">识别结果</span><input v-model="captchaCode" class="field py-2 text-center tracking-widest" maxlength="12" placeholder="自动识别" /></label>
            </div>
            <button class="primary-button mt-3 w-full" :disabled="bookingBusy || !selectedWid" @click="reserveOnce">{{ bookingBusy ? "正在提交…" : "使用此验证码预约一次" }}</button>
          </div>

          <div class="rounded-2xl bg-white p-5 shadow-soft">
            <div class="flex items-center justify-between"><div><h2 class="font-bold">定时自动重试</h2><p class="mt-1 text-xs text-muted">自动识别新验证码并重试</p></div><span v-if="running" class="rounded-full bg-green-50 px-3 py-2 text-xs font-semibold text-green-700">{{ stopping ? "正在停止" : "运行中" }}</span></div>
            <label class="mt-4 block text-xs font-semibold text-muted">预约开放时间</label><input v-model="scheduledAt" class="field mt-2" type="datetime-local" />
            <div class="mt-3 grid grid-cols-2 gap-3"><label class="block text-xs font-semibold text-muted">提前登录（秒）<input v-model="reloginBeforeSeconds" class="field mt-2" type="number" min="0" max="300" step="0.5" /></label><label class="block text-xs font-semibold text-muted">微调秒数（负数可提前）<input v-model="delaySeconds" class="field mt-2" type="number" min="-60" max="60" step="0.1" /></label></div>
            <button v-if="running" class="soft-button mt-3 w-full text-red-600" :disabled="stopping" @click="stopBooking">{{ stopping ? "正在停止自动重试…" : "停止自动重试" }}</button>
            <button v-else class="soft-button mt-3 w-full" :disabled="!selectedWid" @click="startBooking(scheduledAt ? new Date(scheduledAt).getTime() : Date.now())">按设定时间开始</button>
          </div>

          <div class="rounded-2xl bg-white shadow-soft">
            <div class="flex items-center justify-between border-b border-gray-100 px-5 py-4"><div><h2 class="font-bold">运行记录</h2><p class="mt-1 text-xs text-muted">完整记录保存在本机</p></div><div class="flex gap-3"><button class="text-xs font-semibold text-brand" @click="openBookingLog">打开本地日志</button><button class="text-xs font-semibold text-muted" @click="clearLogs">清空</button></div></div>
            <div class="max-h-48 overflow-auto px-5 py-2"><div v-if="!logs.length" class="py-5 text-center text-xs text-muted">预约日志会显示在这里</div><div v-for="(log, index) in logs" :key="index" class="flex gap-3 border-b border-gray-50 py-2.5 text-xs"><span class="shrink-0 font-mono text-muted">{{ log.at }}</span><span :class="log.level === 'success' ? 'text-green-700' : log.level === 'error' ? 'text-red-600' : 'text-muted'">{{ log.message }}</span></div></div>
          </div>
        </aside>
      </section>
      <footer class="mt-6 flex justify-between border-t border-gray-200 pt-4 text-xs text-muted"><span>Fetch Lecture · Tauri 桌面端</span><span>验证码由本机推理；预约结果以学校系统为准。</span></footer>
    <div v-if="reauthRequired" class="fixed inset-0 z-50 flex items-center justify-center bg-gray-900 bg-opacity-40 px-5">
      <section class="w-full max-w-md rounded-2xl bg-white p-6 shadow-card">
        <div class="text-xs font-bold uppercase tracking-widest text-brand">Session refresh</div>
        <h2 class="mt-2 text-xl font-bold">预约前需要短信验证</h2>
        <p class="mt-2 text-sm leading-6 text-muted">学校要求重新确认此设备。完成验证后，定时预约会自动继续。</p>
        <button class="soft-button mt-4 w-full" type="button" :disabled="busy" @click="sendSms">{{ busy ? "正在发送…" : "发送短信验证码" }}</button>
        <form class="mt-3 space-y-3" @submit.prevent="verifyPhone">
          <input v-model="phoneCode" class="field tracking-widest" inputmode="numeric" autocomplete="one-time-code" placeholder="输入收到的验证码" />
          <button class="primary-button w-full" type="submit" :disabled="busy">{{ busy ? "正在验证…" : "验证并继续预约" }}</button>
        </form>
        <p v-if="error" class="mt-3 rounded-xl bg-red-50 px-4 py-3 text-sm text-red-700">{{ error }}</p>
        <p v-else-if="notice" class="mt-3 rounded-xl bg-blue-50 px-4 py-3 text-sm text-blue-800">{{ notice }}</p>
        <button class="mt-4 w-full py-2 text-sm font-semibold text-muted" type="button" @click="stopBooking">停止预约</button>
      </section>
    </div>
    </main>
  </div>
</template>
