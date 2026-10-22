<script setup>
import { ref, onMounted, onUnmounted, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";

const info = ref(null);
const settings = ref(null);
const events = ref([]);
const remindStatus = ref(null);
const error = ref("");
const toast = ref("");

// M7 日历视图
const activeTab = ref("calendar"); // "calendar" | "manage" | "settings"
const calYear = ref(new Date().getFullYear());
const calMonth = ref(new Date().getMonth()); // 0-indexed
const selectedDate = ref(""); // YYYY-MM-DD

const WEEKDAYS = ["日", "一", "二", "三", "四", "五", "六"];

const calMonthLabel = computed(() => `${calYear.value}年${calMonth.value + 1}月`);

const calWeeks = computed(() => {
  const y = calYear.value, m = calMonth.value;
  const firstDay = new Date(y, m, 1).getDay(); // 0=Sun
  const daysInMonth = new Date(y, m + 1, 0).getDate();
  const cells = [];
  // 前置空白
  for (let i = 0; i < firstDay; i++) cells.push(null);
  for (let d = 1; d <= daysInMonth; d++) {
    const dateStr = `${y}-${String(m + 1).padStart(2, "0")}-${String(d).padStart(2, "0")}`;
    cells.push({ day: d, date: dateStr });
  }
  // 后置空白补齐到整周
  while (cells.length % 7 !== 0) cells.push(null);
  // 按周分组
  const weeks = [];
  for (let i = 0; i < cells.length; i += 7) {
    weeks.push(cells.slice(i, i + 7));
  }
  return weeks;
});

function eventsForDate(dateStr) {
  return events.value.filter(ev => dateStr >= ev.startDate && dateStr <= ev.endDate);
}

function isToday(dateStr) {
  const d = new Date();
  const today = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  return dateStr === today;
}

function prevMonth() {
  if (calMonth.value === 0) { calMonth.value = 11; calYear.value--; }
  else calMonth.value--;
  selectedDate.value = "";
}

function nextMonth() {
  if (calMonth.value === 11) { calMonth.value = 0; calYear.value++; }
  else calMonth.value++;
  selectedDate.value = "";
}

function goToToday() {
  const d = new Date();
  calYear.value = d.getFullYear();
  calMonth.value = d.getMonth();
  selectedDate.value = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

function selectDay(cell) {
  if (!cell) return;
  selectedDate.value = selectedDate.value === cell.date ? "" : cell.date;
}

// M4 主题
const theme_image_bytes = ref(null); // ArrayBuffer，传给 Rust 解码
const themeMsg = ref("");
const currentTheme = ref(null);

// M4.2 窗口背景
const bgType = ref("solid"); // "solid" | "image"
const bgColor = ref("#14161a");
const bgImage = ref("");
const bgBlur = ref(24);

// M4.3 时钟
const now = ref(new Date());
let clockTimer = null;

const timeStr = computed(() => {
  const h = String(now.value.getHours()).padStart(2, "0");
  const m = String(now.value.getMinutes()).padStart(2, "0");
  return `${h}:${m}`;
});

const dateStr = computed(() => {
  const d = now.value;
  const weekdays = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];
  const month = d.getMonth() + 1;
  const day = d.getDate();
  const weekday = weekdays[d.getDay()];
  return `${month}月${day}日 ${weekday}`;
});

onMounted(() => {
  clockTimer = setInterval(() => {
    now.value = new Date();
  }, 1000);
});

onUnmounted(() => {
  if (clockTimer) clearInterval(clockTimer);
});

function todayStr() {
  const d = new Date();
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

// 新建事件表单
const form = ref({ title: "", startDate: todayStr(), startTime: "", endDate: "", recurrence: "", recurrenceDaysOfWeek: [], recurrenceUntil: "" });

// 文本解析（M3）
const parseSource = ref("");
const parseItems = ref([]);
const parseMsg = ref("");
const parsing = ref(false);

// M6 OCR
const ocrText = ref("");
const ocrMsg = ref("");
const ocrBusy = ref(false);

async function ocrFromClipboard() {
  ocrMsg.value = "识别中…";
  ocrBusy.value = true;
  try {
    ocrText.value = await invoke("ocr_clipboard_text");
    ocrMsg.value = "识别完成";
  } catch (e) {
    ocrMsg.value = `识别失败：${e}`;
  } finally {
    ocrBusy.value = false;
  }
}

function ocrOnFileSelect(e) {
  const file = e.target.files?.[0];
  if (!file) return;
  ocrMsg.value = "识别中…";
  ocrBusy.value = true;
  const reader = new FileReader();
  reader.onload = async () => {
    try {
      const bytes = Array.from(new Uint8Array(reader.result));
      ocrText.value = await invoke("ocr_extract_bytes", { imageBytes: bytes });
      ocrMsg.value = "识别完成";
    } catch (e) {
      ocrMsg.value = `识别失败：${e}`;
    } finally {
      ocrBusy.value = false;
    }
  };
  reader.readAsArrayBuffer(file);
}

function ocrSendToParse() {
  parseSource.value = ocrText.value;
  ocrText.value = "";
  ocrMsg.value = "已填入解析框";
}

async function loadAll() {
  error.value = "";
  try {
    info.value = await invoke("app_info");
    settings.value = await invoke("settings_get");
    events.value = await invoke("events_list", { from: null, to: null });
    remindStatus.value = await invoke("reminders_status");
    // M4 加载持久化主题
    const savedTheme = await invoke("theme_load");
    if (savedTheme) {
      currentTheme.value = savedTheme;
      applyThemeTokens(savedTheme);
    }
  } catch (e) {
    error.value = String(e);
  }
}

// M6.2 重复事件显示辅助
const RECURRENCE_LABELS = { daily: "每天", weekly: "每周", monthly: "每月", yearly: "每年" };
const DOW_SHORT = ["日", "一", "二", "三", "四", "五", "六"];
function recurrenceShort(ev) {
  if (!ev.recurrence) return "";
  const base = RECURRENCE_LABELS[ev.recurrence] || ev.recurrence;
  if (ev.recurrence === "weekly" && ev.recurrenceDaysOfWeek?.length) {
    return base + "(" + ev.recurrenceDaysOfWeek.map(d => DOW_SHORT[d] || d).join("") + ")";
  }
  return base;
}
function recurrenceLabel(ev) {
  let label = recurrenceShort(ev);
  if (ev.recurrenceUntil) label += ` 至 ${ev.recurrenceUntil}`;
  return label;
}

async function addEvent() {
  error.value = "";
  try {
    const payload = {
      title: form.value.title,
      startDate: form.value.startDate,
      startTime: form.value.startTime || null,
      endDate: form.value.endDate || null,
    };
    // M6.2 重复事件
    if (form.value.recurrence) {
      payload.recurrence = form.value.recurrence;
      if (form.value.recurrence === "weekly" && form.value.recurrenceDaysOfWeek.length > 0) {
        payload.recurrenceDaysOfWeek = form.value.recurrenceDaysOfWeek;
      }
      if (form.value.recurrenceUntil) {
        payload.recurrenceUntil = form.value.recurrenceUntil;
      }
    }
    await invoke("events_create", { payload });
    form.value.title = "";
    form.value.startTime = "";
    form.value.endDate = "";
    form.value.recurrence = "";
    form.value.recurrenceDaysOfWeek = [];
    form.value.recurrenceUntil = "";
    await loadAll();
  } catch (e) {
    error.value = String(e);
  }
}

// 日历视图快速添加事件（添加到选中的日期）
async function addEventForDate() {
  if (!form.value.title.trim() || !selectedDate.value) return;
  try {
    await invoke("events_create", {
      payload: {
        title: form.value.title.trim(),
        startDate: selectedDate.value,
        startTime: null,
        endDate: selectedDate.value,
      },
    });
    form.value.title = "";
    await loadAll();
  } catch (e) {
    error.value = String(e);
  }
}

async function toggleCompleted(ev) {
  try {
    await invoke("events_set_completed", { id: ev.id, completed: !ev.completed });
    await loadAll();
  } catch (e) {
    error.value = String(e);
  }
}

async function removeEvent(ev) {
  try {
    await invoke("events_remove", { id: ev.id });
    await loadAll();
  } catch (e) {
    error.value = String(e);
  }
}

async function saveSettings() {
  error.value = "";
  try {
    await invoke("settings_update", {
      patch: {
        defaultRemindTime: settings.value.defaultRemindTime,
        catchUpOnStart: settings.value.catchUpOnStart,
        autoLaunch: settings.value.autoLaunch,
        aiParseEnabled: settings.value.aiParseEnabled,
      },
    });
    toast.value = "设置已保存";
    setTimeout(() => (toast.value = ""), 1600);
    await loadAll();
  } catch (e) {
    error.value = String(e);
  }
}

// ———— M4 主题系统 ————

function onThemeImageSelect(e) {
  const file = e.target.files?.[0];
  if (!file) return;
  // WebView2 不暴露 file.path，改用 FileReader 读字节直传 Rust
  const reader = new FileReader();
  reader.onload = () => {
    theme_image_bytes.value = new Uint8Array(reader.result);
    themeMsg.value = "已选择图片，点击应用提取主题";
  };
  reader.readAsArrayBuffer(file);
}

// M4 主题令牌应用
function applyThemeTokens(tokens) {
  const root = document.documentElement;
  root.style.setProperty("--primary", tokens.primary);
  root.style.setProperty("--primary-strong", tokens.primaryStrong);
  root.style.setProperty("--accent", tokens.accent);
  root.style.setProperty("--bg", tokens.bg);
  root.style.setProperty("--surface", tokens.surface);
  root.style.setProperty("--text", tokens.text);
  root.style.setProperty("--text-sub", tokens.textSub);
  root.style.setProperty("--border", tokens.border);
  root.style.setProperty("--mode", tokens.mode);
}

async function applyExtractedTheme() {
  if (!theme_image_bytes.value) return;
  themeMsg.value = "提取中...";
  try {
    const tokens = await invoke("theme_extract_colors", { imageBytes: Array.from(theme_image_bytes.value) });
    currentTheme.value = tokens;
    applyThemeTokens(tokens);
    // 保存主题
    await invoke("theme_save", { tokens });
    themeMsg.value = `已应用并保存 ${tokens.mode} 主题`;
  } catch (e) {
    themeMsg.value = `提取失败：${e}`;
  }
}

// M4.2 窗口背景
function onBgImageSelect(e) {
  const file = e.target.files?.[0];
  if (!file) return;
  bgImage.value = URL.createObjectURL(file);
  applyBackground();
}

function applyBackground() {
  const body = document.body;
  if (bgType.value === "solid") {
    body.style.background = bgColor.value;
    body.style.backgroundImage = "";
  } else if (bgType.value === "image" && bgImage.value) {
    body.style.background = `url(${bgImage.value}) center/cover no-repeat`;
    body.style.filter = `blur(${bgBlur.value}px)`;
  }
}

// ———— M3 文本解析流程：粘贴 → 解析 → 逐字段可改/逐条勾选 → 导入 ————

async function doParse() {
  parseMsg.value = "";
  parsing.value = true;
  try {
    const items = await invoke("parse_text", { text: parseSource.value });
    parseItems.value = items.map((it) => ({
      ...it,
      selected: true,
      startTime: it.startTime || "",
    }));
    if (!items.length) parseMsg.value = "没有解析出可导入的活动";
  } catch (e) {
    parseMsg.value = String(e);
  } finally {
    parsing.value = false;
  }
}

async function importSelected() {
  parseMsg.value = "";
  const sel = parseItems.value.filter((i) => i.selected);
  if (!sel.length) {
    parseMsg.value = "请先勾选要导入的条目";
    return;
  }
  const ready = sel.filter((i) => i.startDate);
  const missing = sel.length - ready.length;
  let ok = 0;
  try {
    for (const it of ready) {
      await invoke("events_create", {
        payload: {
          title: it.title.trim() || "未命名活动",
          startDate: it.startDate,
          startTime: it.startTime || null,
          endDate: it.endDate || it.startDate,
          sourceText: parseSource.value,
          sourceFragment: it.fragment,
        },
      });
      ok++;
    }
  } catch (e) {
    parseMsg.value = `导入中断：${e}`;
    await loadAll();
    return;
  }
  parseMsg.value = `已导入 ${ok} 条${missing ? `；${missing} 条未补日期，已跳过` : ""}`;
  parseItems.value = [];
  await loadAll();
}

onMounted(loadAll);
</script>

<template>
  <div class="page">
    <!-- 顶部导航 -->
    <header>
      <div class="header-top">
        <h1>桌面日历</h1>
        <div class="clock-mini">{{ timeStr }}</div>
      </div>
      <nav class="tabs">
        <button :class="{ active: activeTab === 'calendar' }" @click="activeTab = 'calendar'">日历</button>
        <button :class="{ active: activeTab === 'manage' }" @click="activeTab = 'manage'">事件管理</button>
        <button :class="{ active: activeTab === 'settings' }" @click="activeTab = 'settings'">设置</button>
      </nav>
    </header>

    <p v-if="error" class="error">{{ error }}</p>
    <p v-if="toast" class="toast">{{ toast }}</p>

    <!-- ====== 日历 Tab ====== -->
    <div v-if="activeTab === 'calendar'" class="cal-layout">
      <!-- 月视图网格 -->
      <section class="card cal-card">
        <div class="cal-header">
          <button class="cal-nav" @click="prevMonth">◀</button>
          <h2 class="cal-title">{{ calMonthLabel }}</h2>
          <button class="cal-nav" @click="nextMonth">▶</button>
          <button class="cal-today" @click="goToToday">今天</button>
        </div>
        <div class="cal-grid">
          <div class="cal-wd" v-for="wd in WEEKDAYS" :key="wd">{{ wd }}</div>
          <template v-for="(week, wi) in calWeeks" :key="wi">
            <div
              v-for="(cell, ci) in week" :key="ci"
              class="cal-cell"
              :class="{
                'cal-empty': !cell,
                'cal-today': cell && isToday(cell.date),
                'cal-selected': cell && cell.date === selectedDate,
                'cal-has-events': cell && eventsForDate(cell.date).length > 0,
              }"
              @click="selectDay(cell)"
            >
              <template v-if="cell">
                <span class="cal-day-num">{{ cell.day }}</span>
                <div class="cal-dots">
                  <span
                    v-for="ev in eventsForDate(cell.date).slice(0, 3)"
                    :key="ev.id"
                    class="cal-dot"
                    :class="{ 'cal-dot-done': ev.completed }"
                    :title="ev.title"
                  ></span>
                  <span v-if="eventsForDate(cell.date).length > 3" class="cal-more">+{{ eventsForDate(cell.date).length - 3 }}</span>
                </div>
              </template>
            </div>
          </template>
        </div>
      </section>

      <!-- 日详情侧栏 -->
      <section v-if="selectedDate" class="card day-detail">
        <h2>{{ selectedDate }} 的事件</h2>
        <div v-if="eventsForDate(selectedDate).length === 0" class="muted">当天没有事件</div>
        <div v-for="ev in eventsForDate(selectedDate)" :key="ev.id" class="day-ev">
          <div class="day-ev-top">
            <input type="checkbox" :checked="ev.completed" @change="toggleCompleted(ev)" />
            <span :class="{ done: ev.completed }">{{ ev.title }}</span>
            <span class="muted">{{ ev.startTime || "全天" }}</span>
          </div>
          <div class="day-ev-bottom">
            <span v-if="ev.recurrence" class="recurrence-badge">{{ recurrenceShort(ev) }}</span>
            <button class="ghost" @click="removeEvent(ev)">删除</button>
          </div>
        </div>
        <!-- 快速新建 -->
        <div class="day-quick-add">
          <input v-model="form.title" placeholder="快速添加事件…" @keyup.enter="addEventForDate" />
          <button @click="addEventForDate" :disabled="!form.title.trim()">+</button>
        </div>
      </section>

      <!-- 提醒状态 -->
      <div v-if="remindStatus" class="muted cal-status">
        {{ remindStatus.pending }} 条待处理提醒<template v-if="remindStatus.nextPlannedAt">｜下次 {{ remindStatus.nextPlannedAt.replace("T", " ") }}</template>
      </div>
    </div>

    <!-- ====== 事件管理 Tab ====== -->
    <div v-if="activeTab === 'manage'">
      <!-- 新建事件 -->
      <section class="card">
        <h2>新建事件</h2>
        <div class="grid">
          <label class="wide">标题<input v-model="form.title" placeholder="例如：整理团日活动材料" /></label>
          <label>开始日期<input type="date" v-model="form.startDate" /></label>
          <label>开始时间（空 = 全天）<input v-model="form.startTime" placeholder="14:30" /></label>
          <label>结束日期（空 = 同开始）<input type="date" v-model="form.endDate" /></label>
          <label>重复
            <select v-model="form.recurrence">
              <option value="">不重复</option>
              <option value="daily">每天</option>
              <option value="weekly">每周</option>
              <option value="monthly">每月</option>
              <option value="yearly">每年</option>
            </select>
          </label>
          <label v-if="form.recurrence === 'weekly'" class="wide">星期（可多选）
            <div class="dow-group">
              <label v-for="(name, idx) in ['日','一','二','三','四','五','六']" :key="idx" class="dow-check">
                <input type="checkbox" :value="idx" v-model="form.recurrenceDaysOfWeek" />{{ name }}
              </label>
            </div>
          </label>
          <label v-if="form.recurrence">重复截止<input type="date" v-model="form.recurrenceUntil" /></label>
          <button @click="addEvent">添加</button>
        </div>
      </section>

      <!-- OCR -->
      <section class="card">
        <h2>截图识别（OCR → 解析）</h2>
        <div class="ocr-actions">
          <button @click="ocrFromClipboard" :disabled="ocrBusy">
            {{ ocrBusy ? "识别中…" : "从剪贴板识别" }}
          </button>
          <label class="ocr-file-btn">
            从图片文件识别
            <input type="file" accept="image/*" @change="ocrOnFileSelect" :disabled="ocrBusy" />
          </label>
          <span v-if="ocrMsg" class="muted">{{ ocrMsg }}</span>
        </div>
        <textarea v-if="ocrText" v-model="ocrText" rows="4" placeholder="识别结果…"></textarea>
        <button v-if="ocrText" @click="ocrSendToParse">发送到文本解析</button>
      </section>

      <!-- 文本解析 -->
      <section class="card">
        <h2>文本解析（粘贴 → 解析 → 勾选导入）</h2>
        <textarea v-model="parseSource" rows="4" placeholder="粘贴一段话，例如：&#10;1、10月19日（周一）晚7点团日活动&#10;2、下周三下午2:30 部门例会"></textarea>
        <div class="parse-actions">
          <button @click="doParse" :disabled="parsing || !parseSource.trim()">{{ parsing ? "解析中…" : "解析" }}</button>
          <span v-if="parseMsg" class="muted">{{ parseMsg }}</span>
        </div>
        <div v-if="parseItems.length" class="parsed-list">
          <div v-for="(it, i) in parseItems" :key="i" class="parsed-item">
            <label class="pi-check"><input type="checkbox" v-model="it.selected" /></label>
            <div class="pi-fields">
              <input class="pi-title" v-model="it.title" placeholder="活动名" />
              <div class="pi-row">
                <input type="date" v-model="it.startDate" />
                <input type="time" v-model="it.startTime" />
                <span class="muted">至</span>
                <input type="date" v-model="it.endDate" />
              </div>
              <p v-if="it.warnings.length" class="pi-warn">{{ it.warnings.join("；") }}</p>
              <p class="muted">原文：{{ it.fragment }}</p>
            </div>
          </div>
          <button @click="importSelected">导入选中（{{ parseItems.filter((i) => i.selected).length }} 条）</button>
        </div>
      </section>

      <!-- 事件列表 -->
      <section class="card">
        <h2>全部事件（{{ events.length }} 条）</h2>
        <table v-if="events.length">
          <thead><tr><th>完成</th><th>标题</th><th>日期</th><th>时间</th><th></th></tr></thead>
          <tbody>
            <tr v-for="ev in events" :key="ev.id">
              <td><input type="checkbox" :checked="ev.completed" @change="toggleCompleted(ev)" /></td>
              <td :class="{ done: ev.completed }">{{ ev.title }}</td>
              <td>
                {{ ev.startDate }}<template v-if="ev.endDate !== ev.startDate"> ~ {{ ev.endDate }}</template>
                <span v-if="ev.recurrence" class="recurrence-badge" :title="recurrenceLabel(ev)">{{ recurrenceShort(ev) }}</span>
              </td>
              <td>{{ ev.startTime || "全天" }}</td>
              <td><button class="ghost" @click="removeEvent(ev)">删除</button></td>
            </tr>
          </tbody>
        </table>
        <p v-else class="muted">暂无事件</p>
      </section>
    </div>

    <!-- ====== 设置 Tab ====== -->
    <div v-if="activeTab === 'settings'">
      <!-- 时钟 -->
      <section class="card clock-card">
        <div class="clock-time">{{ timeStr }}</div>
        <div class="clock-date">{{ dateStr }}</div>
      </section>

      <!-- 设置 -->
      <section class="card">
        <h2>提醒与启动</h2>
        <div v-if="settings" class="grid">
          <label>默认提醒时刻<input v-model="settings.defaultRemindTime" placeholder="07:30" /></label>
          <label class="check"><input type="checkbox" v-model="settings.catchUpOnStart" />开机补提醒</label>
          <label class="check"><input type="checkbox" v-model="settings.autoLaunch" />开机自启</label>
          <button @click="saveSettings">保存设置</button>
        </div>
      </section>

      <!-- 主题 -->
      <section class="card">
        <h2>照片换肤</h2>
        <div class="theme-upload">
          <input type="file" accept="image/*" @change="onThemeImageSelect" />
          <button @click="applyExtractedTheme" :disabled="!theme_image_bytes">应用提取的主题</button>
          <span v-if="themeMsg" class="muted">{{ themeMsg }}</span>
        </div>
        <div v-if="currentTheme" class="theme-preview">
          <div class="theme-swatch" :style="{ background: currentTheme.primary }" title="主色"></div>
          <div class="theme-swatch" :style="{ background: currentTheme.accent }" title="强调色"></div>
          <div class="theme-swatch" :style="{ background: currentTheme.bg }" title="背景"></div>
          <span class="muted">当前：{{ currentTheme.mode }} 模式</span>
        </div>
        <div class="bg-settings">
          <label class="muted">窗口背景：</label>
          <select v-model="bgType" @change="applyBackground">
            <option value="solid">纯色</option>
            <option value="image">图片</option>
          </select>
          <input v-if="bgType === 'solid'" type="color" v-model="bgColor" @change="applyBackground" />
          <input v-if="bgType === 'image'" type="file" accept="image/*" @change="onBgImageSelect" />
          <label v-if="bgType === 'image'" class="muted">
            模糊度：<input type="range" v-model.number="bgBlur" min="0" max="50" @change="applyBackground" />
          </label>
        </div>
      </section>

      <!-- 花园 -->
      <section class="card garden-card">
        <h2>小花园</h2>
        <div class="garden-content">
          <div class="garden-plant">🌱</div>
          <div class="garden-stats">
            <div class="garden-stat"><span class="garden-value">0</span><span class="garden-label">浇水量</span></div>
            <div class="garden-stat"><span class="garden-value">0</span><span class="garden-label">连续天数</span></div>
          </div>
          <p class="muted">完成当天所有任务后，小树 +1 滴水</p>
        </div>
      </section>

      <footer class="muted">
        v{{ info?.version }} · {{ info?.profile }} · 数据目录：{{ info?.dataDir }}
      </footer>
    </div>
  </div>
</template>

<style>
:root {
  /* M4 主题令牌（默认深色） */
  --primary: #4a90e2;
  --primary-strong: #5a9ff2;
  --accent: #6ab0f2;
  --bg: #14161a;
  --surface: #1b1e23;
  --text: #e8eaed;
  --text-sub: #9aa0a6;
  --border: #2a2e35;
  --mode: dark;

  font-family: "Segoe UI", "Microsoft YaHei UI", system-ui, sans-serif;
  font-size: 14px;
  color-scheme: dark;
}
* {
  box-sizing: border-box;
}
body {
  margin: 0;
  background: var(--bg);
  color: var(--text);
  transition: background 0.4s, color 0.4s;
}
.page {
  max-width: 920px;
  margin: 0 auto;
  padding: 20px 22px 40px;
}
header h1 {
  font-size: 20px;
  margin: 0 0 6px;
}
.meta {
  display: flex;
  gap: 12px;
  align-items: center;
  color: #9aa0a6;
  font-size: 12px;
  flex-wrap: wrap;
}
.meta code {
  background: var(--surface);
  padding: 2px 8px;
  border-radius: 6px;
}
.card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 14px 16px;
  margin-top: 14px;
}
.card h2 {
  font-size: 14px;
  margin: 0 0 10px;
  color: var(--text-sub);
  font-weight: 600;
}
.grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(190px, 1fr));
  gap: 10px 14px;
  align-items: end;
}
label {
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 12px;
  color: var(--text-sub);
}
label.check {
  flex-direction: row;
  align-items: center;
  gap: 6px;
  padding-bottom: 8px;
}
label.wide {
  grid-column: 1 / -1;
}
input {
  background: var(--bg);
  border: 1px solid var(--border);
  color: var(--text);
  border-radius: 8px;
  padding: 7px 9px;
  font-size: 13px;
}
input[type="checkbox"] {
  width: 15px;
  height: 15px;
  accent-color: var(--primary);
}
textarea {
  width: 100%;
  background: var(--bg);
  border: 1px solid var(--border);
  color: var(--text);
  border-radius: 8px;
  padding: 8px 10px;
  font-size: 13px;
  font-family: inherit;
  resize: vertical;
}
.parse-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 8px;
}
.parsed-list {
  margin-top: 12px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.parsed-item {
  display: flex;
  gap: 10px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 10px 12px;
}
.pi-check {
  padding-top: 6px;
}
.pi-fields {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 7px;
}
.pi-fields input.pi-title {
  width: 100%;
}
.pi-row {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.pi-warn {
  margin: 0;
  color: #e8c468;
  font-size: 12px;
}
.pi-fields p.muted {
  margin: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
button {
  background: var(--primary);
  border: 0;
  color: #fff;
  border-radius: 8px;
  padding: 8px 14px;
  font-size: 13px;
  cursor: pointer;
  transition: background 0.2s;
}
button:hover {
  background: var(--primary-strong);
}
button.ghost {
  background: transparent;
  border: 1px solid var(--border);
  color: var(--text-sub);
  padding: 4px 10px;
}
button.ghost:hover {
  border-color: #a33;
  color: #f28b82;
  background: transparent;
}
table {
  width: 100%;
  border-collapse: collapse;
  font-size: 13px;
}
th,
td {
  text-align: left;
  padding: 7px 8px;
  border-bottom: 1px solid var(--border);
}
th {
  color: var(--text-sub);
  font-weight: 500;
  font-size: 12px;
}
.done {
  text-decoration: line-through;
  color: var(--text-sub);
}
.muted {
  color: var(--text-sub);
  font-size: 12px;
}
.error {
  background: #3a1d1d;
  border: 1px solid #7a2c2c;
  color: #f2b8b5;
  padding: 8px 12px;
  border-radius: 8px;
}
.toast {
  background: #17301d;
  border: 1px solid #2e6b3c;
  color: #a8e6b0;
  padding: 8px 12px;
  border-radius: 8px;
}
footer {
  margin-top: 18px;
}
/* M4 主题 */
.theme-upload {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.theme-upload input[type="file"] {
  font-size: 12px;
  color: var(--text-sub);
}
.theme-preview {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 10px;
}
.theme-swatch {
  width: 28px;
  height: 28px;
  border-radius: 6px;
  border: 1px solid var(--border);
}
.bg-settings {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 12px;
  flex-wrap: wrap;
}
.bg-settings select,
.bg-settings input[type="color"] {
  padding: 4px 8px;
  font-size: 12px;
}
.bg-settings input[type="range"] {
  width: 100px;
}
/* M4.3 时钟 */
.clock-card {
  text-align: center;
  padding: 20px;
}
.clock-time {
  font-size: 52px;
  font-weight: 300;
  color: var(--text);
  letter-spacing: 2px;
}
.clock-date {
  font-size: 14px;
  color: var(--text-sub);
  margin-top: 4px;
}
/* M4.4 花园 */
.garden-card {
  text-align: center;
}
.garden-content {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
}
.garden-plant {
  font-size: 64px;
  line-height: 1;
}
.garden-stats {
  display: flex;
  gap: 24px;
}
.garden-stat {
  display: flex;
  flex-direction: column;
  align-items: center;
}
.garden-value {
  font-size: 24px;
  font-weight: 600;
  color: var(--primary);
}
.garden-label {
  font-size: 12px;
  color: var(--text-sub);
}
.dow-group {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
  margin-top: 4px;
}
.dow-check {
  display: flex;
  align-items: center;
  gap: 2px;
  font-size: 13px;
  cursor: pointer;
}
.dow-check input {
  width: 14px;
  height: 14px;
}
.ocr-actions {
  display: flex;
  gap: 8px;
  align-items: center;
  flex-wrap: wrap;
}
.ocr-file-btn {
  display: inline-block;
  padding: 4px 10px;
  border: 1px solid var(--border);
  border-radius: 4px;
  cursor: pointer;
  font-size: 13px;
  position: relative;
  overflow: hidden;
}
.ocr-file-btn input[type="file"] {
  position: absolute;
  top: 0; left: 0;
  width: 100%; height: 100%;
  opacity: 0;
  cursor: pointer;
}
.recurrence-badge {
  display: inline-block;
  margin-left: 4px;
  padding: 1px 5px;
  font-size: 11px;
  border-radius: 3px;
  background: var(--accent);
  color: var(--surface);
  vertical-align: middle;
}

/* ====== M7 日历视图 ====== */
.header-top {
  display: flex;
  justify-content: space-between;
  align-items: center;
}
.header-top h1 {
  font-size: 20px;
  margin: 0;
}
.clock-mini {
  font-size: 28px;
  font-weight: 700;
  color: var(--primary);
  font-variant-numeric: tabular-nums;
}
.tabs {
  display: flex;
  gap: 4px;
  margin-top: 12px;
  border-bottom: 1px solid var(--border);
  padding-bottom: 0;
}
.tabs button {
  background: none;
  border: none;
  color: var(--text-sub);
  padding: 8px 18px;
  font-size: 14px;
  cursor: pointer;
  border-bottom: 2px solid transparent;
  transition: all 0.2s;
}
.tabs button:hover {
  color: var(--text);
}
.tabs button.active {
  color: var(--primary);
  border-bottom-color: var(--primary);
  font-weight: 600;
}

/* 日历布局 */
.cal-layout {
  display: flex;
  flex-direction: column;
  gap: 14px;
}
.cal-card {
  padding: 14px;
}
.cal-header {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 12px;
}
.cal-title {
  font-size: 18px;
  font-weight: 600;
  margin: 0;
  flex: 1;
  text-align: center;
  color: var(--text);
}
.cal-nav {
  background: none;
  border: 1px solid var(--border);
  color: var(--text-sub);
  width: 32px;
  height: 32px;
  border-radius: 8px;
  cursor: pointer;
  font-size: 14px;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: all 0.15s;
}
.cal-nav:hover {
  background: var(--border);
  color: var(--text);
}
.cal-today {
  background: var(--primary);
  color: #fff;
  border: none;
  padding: 5px 14px;
  border-radius: 6px;
  font-size: 13px;
  cursor: pointer;
  transition: opacity 0.15s;
}
.cal-today:hover {
  opacity: 0.85;
}

/* 日历网格 */
.cal-grid {
  display: grid;
  grid-template-columns: repeat(7, 1fr);
  gap: 2px;
}
.cal-wd {
  text-align: center;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-sub);
  padding: 6px 0;
}
.cal-cell {
  min-height: 68px;
  padding: 4px 6px;
  border-radius: 8px;
  cursor: pointer;
  transition: background 0.15s;
  border: 1px solid transparent;
  position: relative;
}
.cal-cell:hover {
  background: rgba(255,255,255,0.04);
}
.cal-empty {
  cursor: default;
  pointer-events: none;
}
.cal-today {
  border-color: var(--primary);
  background: rgba(74, 144, 226, 0.08);
}
.cal-selected {
  background: rgba(74, 144, 226, 0.15);
  border-color: var(--primary-strong);
}
.cal-day-num {
  font-size: 13px;
  font-weight: 500;
  color: var(--text);
}
.cal-today .cal-day-num {
  color: var(--primary);
  font-weight: 700;
}
.cal-dots {
  display: flex;
  flex-wrap: wrap;
  gap: 3px;
  margin-top: 4px;
}
.cal-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--primary);
}
.cal-dot-done {
  background: var(--text-sub);
  opacity: 0.4;
}
.cal-more {
  font-size: 10px;
  color: var(--text-sub);
}

/* 日详情 */
.day-detail {
  margin-top: 0;
}
.day-detail h2 {
  font-size: 15px;
  color: var(--text);
  margin-bottom: 10px;
}
.day-ev {
  padding: 8px 0;
  border-bottom: 1px solid var(--border);
}
.day-ev:last-of-type {
  border-bottom: none;
}
.day-ev-top {
  display: flex;
  align-items: center;
  gap: 8px;
}
.day-ev-bottom {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 4px;
  padding-left: 23px;
}
.day-quick-add {
  display: flex;
  gap: 6px;
  margin-top: 12px;
}
.day-quick-add input {
  flex: 1;
}
.day-quick-add button {
  background: var(--primary);
  color: #fff;
  border: none;
  width: 34px;
  height: 34px;
  border-radius: 8px;
  font-size: 18px;
  cursor: pointer;
}
.day-quick-add button:disabled {
  opacity: 0.4;
  cursor: default;
}
.cal-status {
  font-size: 12px;
  text-align: center;
  padding: 4px 0;
}
</style>
