"use strict";

const SUPPORTED_LANGUAGES = ["en", "zh", "ru"];
const HTML_LANG = { en: "en", zh: "zh-CN", ru: "ru" };
const I18N_ATTRIBUTES = {
  placeholder: "data-i18n-placeholder",
  title: "data-i18n-title",
  "aria-label": "data-i18n-aria-label",
};

const MESSAGES = {
  en: {
    skipLink: "Skip to firmware records",
    siteNav: "Site navigation",
    haucet: "Haucet",
    language: "Language",
    themeToggleTitle: "Toggle dark / light",
    themeToggleLabel: "Toggle dark mode",
    submitTitle: "Submit a new version",
    brandLabel: "Firmware brand",
    huawei: "Huawei",
    honor: "Honor",
    byVersion: "Submit by version",
    deviceModel: "Device model",
    patchVersion: "Patch version",
    patchEnabledLabel: "Include patch version",
    patchEnabledTitle: "Checked: include the patch version; unchecked: query without patch",
    vendor: "Channel / region",
    baseVersion: "Base system version",
    custVersion: "CUST version",
    preloadVersion: "PRELOAD version",
    packageType: "Package type",
    upgrade: "Incremental",
    recovery: "Full",
    rollback: "Rollback (Beta)",
    submit: "Submit",
    manualTitle: "Submit three packages manually",
    manualUrls: "Firmware package URLs (one per line)",
    manualPlaceholder: "Paste one full ZIP URL per line",
    recordsTitle: "Firmware list",
    totalCountLabel: "Saved records",
    showEmulators: "Show emulator packages",
    filterLabel: "Filter firmware records",
    searchLabel: "Search records",
    searchPlaceholder: "Model, version, channel…",
    allTypes: "All types",
    colDevice: "Device",
    colVersion: "Firmware version",
    colDetails: "Details",
    loading: "Loading saved firmware…",
    paginationLabel: "Firmware record pages",
    badResponse: "Unexpected server response (HTTP {status})",
    requestFailed: "Request failed (HTTP {status})",
    detailsOf: "Details for {dev}",
    submittedAt: "Submitted: {time}",
    honorUaNotice: "Use a browser UA extension to set the User-Agent for update.hihonorcdn.com to curl/8.13.0, then retry. Downloading firmware requires the same UA.",
    emulatorArm64: "ARM64 Emulator",
    emulatorX86: "X86 Emulator",
    expand: "Expand",
    collapse: "Collapse",
    toggleDetails: "{action} details for {dev} {version}",
    noMatchTitle: "No matching firmware",
    noMatchDescription: "Try another model or version, or clear the filters.",
    emptyTitle: "No verified firmware yet",
    emptyDescription: "Submit a device version; firmware that passes verification is saved automatically.",
    pageInfo: "Page {page} / {total}",
    previousPage: "Previous",
    nextPage: "Next",
    invalidList: "Firmware API returned an invalid format",
    loadFailed: "Failed to load: {error}",
    loadErrorTitle: "Unable to load firmware right now",
    loadErrorDescription: "Check your network and reload.",
    reload: "Reload",
    verifying: "Verifying firmware…",
    verifyingManual: "Reading Huawei package versions; the firmware is saved automatically once verified.",
    verifyingBrand: "Verifying firmware with the {brand} server; it is saved automatically once verified.",
    saved: "Firmware verified and saved",
    manualCount: "Enter the 3 URLs for the BASE, CUST and PRELOAD packages, optionally plus updater.zip (3 or 4 URLs in total).",
    manualPrefix: "Only full ZIP URLs starting with http://update.dbankcdn.com/ are supported.",
  },
  zh: {
    skipLink: "跳转到固件记录",
    siteNav: "站点导航",
    haucet: "Haucet",
    language: "语言",
    themeToggleTitle: "切换深色 / 浅色",
    themeToggleLabel: "切换深色模式",
    submitTitle: "提交新的版本",
    brandLabel: "固件品牌",
    huawei: "华为",
    honor: "荣耀",
    byVersion: "根据版本提交",
    deviceModel: "设备型号",
    patchVersion: "补丁版本",
    patchEnabledLabel: "携带补丁版本",
    patchEnabledTitle: "勾选后携带补丁版本；不勾选则查询不带 patch",
    vendor: "渠道 / 地区",
    baseVersion: "基础系统版本",
    custVersion: "定制版本",
    preloadVersion: "预装版本",
    packageType: "包类型",
    upgrade: "差分包",
    recovery: "全量包",
    rollback: "回滚包(Beta)",
    submit: "提交",
    manualTitle: "手动提交三包",
    manualUrls: "固件包链接（每行一个 URL）",
    manualPlaceholder: "每行粘贴一个完整 ZIP 链接",
    recordsTitle: "固件列表",
    totalCountLabel: "已保存记录数",
    showEmulators: "显示模拟器包",
    filterLabel: "筛选固件记录",
    searchLabel: "搜索记录",
    searchPlaceholder: "机型、版本号、渠道…",
    allTypes: "全部类型",
    colDevice: "设备",
    colVersion: "固件版本",
    colDetails: "详细信息",
    loading: "正在加载已保存的固件…",
    paginationLabel: "固件记录分页",
    badResponse: "服务器响应格式异常（HTTP {status}）",
    requestFailed: "请求失败（HTTP {status}）",
    detailsOf: "{dev} 的详细信息",
    submittedAt: "提交时间：{time}",
    honorUaNotice: "请用浏览器 UA 扩展将 update.hihonorcdn.com 的 User-Agent 手动设为 curl/8.13.0 后重试。下载固件也需要相同 UA。",
    emulatorArm64: "ARM64模拟器",
    emulatorX86: "X86模拟器",
    expand: "展开",
    collapse: "收起",
    toggleDetails: "{action} {dev} {version} 的详细信息",
    noMatchTitle: "没有匹配的固件",
    noMatchDescription: "试试其他机型或版本号，或清除筛选条件。",
    emptyTitle: "还没有已验证的固件",
    emptyDescription: "提交设备版本后，验证通过的固件会自动保存。",
    pageInfo: "第 {page} / {total} 页",
    previousPage: "上一页",
    nextPage: "下一页",
    invalidList: "固件接口返回格式不正确",
    loadFailed: "加载失败：{error}",
    loadErrorTitle: "暂时无法加载固件",
    loadErrorDescription: "检查网络后可重新加载。",
    reload: "重新加载",
    verifying: "正在验证固件…",
    verifyingManual: "正在读取华为三包版本信息，验证通过后自动保存。",
    verifyingBrand: "正在向{brand}服务器验证固件，验证通过后自动保存。",
    saved: "固件验证通过，已保存",
    manualCount: "请填写基础包、定制包、预装包的 3 个链接，也可附加 updater.zip，共 3 或 4 个链接。",
    manualPrefix: "仅支持以 http://update.dbankcdn.com/ 开头的完整 ZIP 链接。",
  },
  ru: {
    skipLink: "Перейти к записям прошивок",
    siteNav: "Навигация по сайту",
    haucet: "Haucet",
    language: "Язык",
    themeToggleTitle: "Тёмная / светлая тема",
    themeToggleLabel: "Переключить тёмную тему",
    submitTitle: "Отправить новую версию",
    brandLabel: "Бренд прошивки",
    huawei: "Huawei",
    honor: "Honor",
    byVersion: "Отправка по версии",
    deviceModel: "Модель устройства",
    patchVersion: "Версия патча",
    patchEnabledLabel: "Передавать версию патча",
    patchEnabledTitle: "Отмечено: передавать версию патча; не отмечено: запрос без patch",
    vendor: "Канал / регион",
    baseVersion: "Версия базовой системы",
    custVersion: "Версия CUST",
    preloadVersion: "Версия PRELOAD",
    packageType: "Тип пакета",
    upgrade: "Инкрементальный",
    recovery: "Полный",
    rollback: "Откат (Beta)",
    submit: "Отправить",
    manualTitle: "Ручная отправка трёх пакетов",
    manualUrls: "Ссылки на пакеты прошивки (по одной на строку)",
    manualPlaceholder: "Вставьте по одной полной ссылке на ZIP в строку",
    recordsTitle: "Список прошивок",
    totalCountLabel: "Сохранённых записей",
    showEmulators: "Показывать пакеты эмулятора",
    filterLabel: "Фильтр записей прошивок",
    searchLabel: "Поиск записей",
    searchPlaceholder: "Модель, версия, канал…",
    allTypes: "Все типы",
    colDevice: "Устройство",
    colVersion: "Версия прошивки",
    colDetails: "Подробности",
    loading: "Загрузка сохранённых прошивок…",
    paginationLabel: "Страницы записей прошивок",
    badResponse: "Неожиданный ответ сервера (HTTP {status})",
    requestFailed: "Ошибка запроса (HTTP {status})",
    detailsOf: "Подробности: {dev}",
    submittedAt: "Отправлено: {time}",
    honorUaNotice: "С помощью расширения браузера установите User-Agent для update.hihonorcdn.com в curl/8.13.0 и повторите попытку. Для загрузки прошивки нужен тот же UA.",
    emulatorArm64: "Эмулятор ARM64",
    emulatorX86: "Эмулятор X86",
    expand: "Развернуть",
    collapse: "Свернуть",
    toggleDetails: "{action}: подробности {dev} {version}",
    noMatchTitle: "Подходящих прошивок нет",
    noMatchDescription: "Попробуйте другую модель или версию либо сбросьте фильтры.",
    emptyTitle: "Проверенных прошивок пока нет",
    emptyDescription: "Отправьте версию устройства — прошивки, прошедшие проверку, сохраняются автоматически.",
    pageInfo: "Страница {page} / {total}",
    previousPage: "Назад",
    nextPage: "Вперёд",
    invalidList: "API прошивок вернул данные в неверном формате",
    loadFailed: "Ошибка загрузки: {error}",
    loadErrorTitle: "Не удалось загрузить прошивки",
    loadErrorDescription: "Проверьте подключение и повторите загрузку.",
    reload: "Загрузить снова",
    verifying: "Проверка прошивки…",
    verifyingManual: "Чтение версий трёх пакетов Huawei; после проверки прошивка сохранится автоматически.",
    verifyingBrand: "Проверка прошивки на сервере {brand}; после проверки она сохранится автоматически.",
    saved: "Прошивка проверена и сохранена",
    manualCount: "Укажите 3 ссылки на пакеты BASE, CUST и PRELOAD, при необходимости добавьте updater.zip (всего 3 или 4 ссылки).",
    manualPrefix: "Поддерживаются только полные ссылки на ZIP, начинающиеся с http://update.dbankcdn.com/.",
  },
};

let currentLanguage = detectLanguage();

function detectLanguage() {
  try {
    const stored = localStorage.getItem("lang");
    if (SUPPORTED_LANGUAGES.includes(stored)) return stored;
  } catch (e) {}
  const preferred = navigator.languages && navigator.languages.length ? navigator.languages : [navigator.language];
  for (const tag of preferred) {
    const primary = String(tag || "").toLowerCase().split("-")[0];
    if (SUPPORTED_LANGUAGES.includes(primary)) return primary;
  }
  return "en";
}

function t(key, params = {}) {
  const template = String(MESSAGES[currentLanguage][key] ?? MESSAGES.en[key] ?? key);
  return template.replace(/\{(\w+)\}/g, (match, name) => (name in params ? String(params[name]) : match));
}

function localizedError(key, params) {
  const error = new Error(t(key, params));
  error.i18n = { key, params };
  return error;
}

function errorText(error) {
  return error.i18n ? t(error.i18n.key, error.i18n.params) : error.message;
}

function setLanguage(language, persist) {
  currentLanguage = SUPPORTED_LANGUAGES.includes(language) ? language : "en";
  if (persist) {
    try { localStorage.setItem("lang", currentLanguage); } catch (e) {}
  }
  document.documentElement.lang = HTML_LANG[currentLanguage];
  for (const node of document.querySelectorAll("[data-i18n]")) node.textContent = t(node.dataset.i18n);
  for (const [attribute, dataAttribute] of Object.entries(I18N_ATTRIBUTES)) {
    for (const node of document.querySelectorAll(`[${dataAttribute}]`)) {
      node.setAttribute(attribute, t(node.getAttribute(dataAttribute)));
    }
  }
}
