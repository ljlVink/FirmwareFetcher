"use strict";

const $rows = document.getElementById("rows");
const $msg = document.getElementById("msg");
const $form = document.getElementById("submit-form");
const $submitBtn = document.getElementById("submit-btn");
const $erecoveryEnabled = document.getElementById("erecovery-enabled");
const $manualForm = document.getElementById("manual-submit-form");
const $manualSubmitBtn = document.getElementById("manual-submit-btn");
const $manualUrls = document.getElementById("manual-urls");
const $search = document.getElementById("search");
const $actionFilter = document.getElementById("action-filter");
const $showEmulators = document.getElementById("show-emulators");
const $totalCount = document.getElementById("total-count");
const $recordList = document.getElementById("record-list");
const $autoParse = document.getElementById("auto-parse");
const $vendor = document.getElementById("vendor");
const $patch = document.getElementById("patch");
const $patchEnabled = document.getElementById("patch-enabled");
const $brand = document.getElementById("brand");
const $langSelect = document.getElementById("lang-select");
setLanguage(currentLanguage, false);
$langSelect.value = currentLanguage;
$brand.value = "huawei";
const patchByBrand = { huawei: true, honor: false };
const brandPlaceholders = {
  huawei: Object.fromEntries(["auto-parse", "dev", "base", "cust", "preload"].map(id => [id, document.getElementById(id).placeholder])),
  honor: {
    "auto-parse": "WKL-AN80-LGRP1 11.0.0.111(SP7C00E110R101P2)",
    dev: "WKL-AN80",
    base: "WKL-LGRP1-CHN 11.0.0.111(SP7)",
    cust: "WKL-AN80-CUST 11.0.0.110(C00)",
    preload: "WKL-AN80-PRELOAD 11.0.0.2(C00R101)",
  },
};
let selectedBrand = "huawei";
let activeSubmissions = 0;
let vendorBeforeAutoParse = null;
const AUTO_PARSE_RE = /^([A-Z0-9]+)-([A-Z0-9]+)-(LGRP\d+)\s+(\d+(?:\.\d+){2,3})\((?:SP(\d+))?([A-Z]*C\d+)E(\d+)R(\d+)P(\d+)(?:PATCH(\d+))?\)$/;
const REGION_RULES = {
  C00: { vendor: "all_cn", baseSuffix: "CHN" },
  C01: { vendor: "cmcc_cn", baseSuffix: "CHN" },
  C02: { vendor: "vodafone_es", baseSuffix: "OVS" },
  C10: { vendor: "hw_ru", baseSuffix: "OVS" },
  C16: { vendor: "mtn_za", baseSuffix: "OVS" },
  C25: { vendor: "claro_la", baseSuffix: "OVS" },
  C33: { vendor: "altice_all", baseSuffix: "OVS" },
  C34: { vendor: "optus_au", baseSuffix: "OVS" },
  C40: { vendor: "tef_normal", baseSuffix: "OVS" },
  C45: { vendor: "tigo_la", baseSuffix: "OVS" },
  C55: { vendor: "ti_it", baseSuffix: "OVS" },
  C69: { vendor: "telcel_mx", baseSuffix: "OVS" },
  C77: { vendor: "hw_la", baseSuffix: "OVS" },
  C93: { vendor: "channel_th", baseSuffix: "OVS" },
  C94: { vendor: "hw_tr", baseSuffix: "OVS" },
  C106: { vendor: "ice_cr", baseSuffix: "OVS" },
  C109: { vendor: "orange_all", baseSuffix: "OVS" },
  C111: { vendor: "softbank_jp", baseSuffix: "OVS" },
  C130: { vendor: "etcstore_th", baseSuffix: "OVS" },
  C185: { vendor: "hw_meafnaf", baseSuffix: "OVS" },
  C212: { vendor: "iusacell_mx", baseSuffix: "OVS" },
  C301: { vendor: "channel_id", baseSuffix: "OVS" },
  C316: { vendor: "vodacom_za", baseSuffix: "OVS" },
  C346: { vendor: "vha_au", baseSuffix: "OVS" },
  C431: { vendor: "hw_eea", baseSuffix: "OVS" },
  C432: { vendor: "hw_eu", baseSuffix: "OVS" },
  C451: { vendor: "telstra_au", baseSuffix: "OVS" },
  C461: { vendor: "hw_cea", baseSuffix: "OVS" },
  C521: { vendor: "telefonica_mx", baseSuffix: "OVS" },
  C555: { vendor: "h3g_ie", baseSuffix: "OVS" },
  C578: { vendor: "hw_ca", baseSuffix: "OVS" },
  C605: { vendor: "hw_la", baseSuffix: "OVS" },
  C635: { vendor: "hw_jp", baseSuffix: "OVS" },
  C636: { vendor: "hw_spcseas", baseSuffix: "OVS" },
  C675: { vendor: "hw_in", baseSuffix: "OVS" },
  C700: { vendor: "all_cn", baseSuffix: "CHN" },
  C721: { vendor: "porsche_normal", baseSuffix: "OVS" },
  C735: { vendor: "demo_cn", baseSuffix: "CHN" },
  DEMC735: { vendor: "demo_cn", baseSuffix: "CHN" },
  C771: { vendor: "hw_la", baseSuffix: "OVS" },
  C781: { vendor: "hw_eu", baseSuffix: "OVS" },
  C782: { vendor: "hw_eu", baseSuffix: "OVS" },
  C786: { vendor: "porsche_cn", baseSuffix: "CHN" },
  C792: { vendor: "telus_ca_c792", baseSuffix: "OVS" },
  C800: { vendor: "softbankcorp_jp", baseSuffix: "OVS" },
  C828: { vendor: "porsche_ru", baseSuffix: "OVS" },
  C900: { vendor: "hw_normal", baseSuffix: "OVS" },
};
const ACTION_LABEL_KEY = { upgrade: "upgrade", recovery: "recovery", rollback: "rollback" };
const EMULATER_LABEL_KEY = { arm64: "emulatorArm64", x86: "emulatorX86" };
const SOURCE_LABEL_KEY = {
  manual_version: "sourceManualVersion",
  erec_three_package: "sourceErecThreePackage",
  auto_refresh: "sourceAutoRefresh",
  ffusersubmission: "sourceFfusersubmission",
  admin_manual: "sourceAdminManual",
};
const $themeToggle = document.getElementById("theme-toggle");
const THEME_COLORS = { dark: "#000000", light: "#ffffff" };
const openDetails = new Set();
const PER_PAGE = 50;
let records = [];
let hasLoaded = false;
let loadErrored = false;
let loadVersion = 0;
let lastSavedId = null;
let currentPage = 1;
let totalPages = 1;
let totalCount = 0;
let searchTimer = 0;

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (key === "text") node.textContent = value;
    else if (key === "class") node.className = value;
    else node.setAttribute(key, value);
  }
  node.append(...children);
  return node;
}

let msgText = "";
let msgError = false;

function showMsg(text, error = false) {
  msgText = text;
  msgError = error;
  $msg.hidden = false;
  paintMsg();
}

function paintMsg() {
  $msg.textContent = typeof msgText === "function" ? msgText() : msgText;
  $msg.className = msgError ? "notice notice-error" : "notice";
}

async function requestJson(path, options = {}) {
  const response = await fetch(path, { cache: "no-store", ...options });
  const text = await response.text();
  let data;
  try {
    data = JSON.parse(text);
  } catch {
    throw localizedError("badResponse", { status: response.status });
  }
  if (!response.ok) throw data.error ? new Error(data.error) : localizedError("requestFailed", { status: response.status });
  return data;
}

function cell(label, className, ...children) {
  return el("td", { "data-label": label, class: className }, el("div", { class: "cell-body" }, ...children));
}

function displayVersion(record) {
  let version = record.targetBlVersionNumber;
  const patches = new Set();
  for (const pkg of record.packages) {
    for (const match of pkg.versionNumber.matchAll(/(?:^|[^a-z])(patch\d+)(?![a-z0-9])/gi)) {
      patches.add(match[1].toLowerCase());
    }
  }
  const existing = new Set(Array.from(version.matchAll(/(?:^|[^a-z])(patch\d+)(?![a-z0-9])/gi), match => match[1].toLowerCase()));
  for (const patch of patches) {
    if (!existing.has(patch)) version += `(${patch})`;
  }
  return version;
}

function detailsRow(record) {
  const row = el("tr", { id: `details-${record.id}`, class: "record-details-row" });
  row.hidden = !openDetails.has(record.id);
  if (row.hidden) return row;
  const honor = record.brand === "honor";
  const links = el("ul", { class: "package-list" });
  for (const pkg of record.packages) {
    let href = pkg.url;
    if (honor) {
      const url = new URL(pkg.url);
      url.protocol = "https:";
      href = url.href;
    }
    links.append(el("li", {}, el("a", {
      href, target: "_blank", rel: "noopener noreferrer", title: href,
      text: honor ? `${pkg.versionNumber}` : pkg.versionNumber,
    })));
  }
  const panel = el("section", { class: "detail-panel", "aria-label": t("detailsOf", { dev: record.dev }) },
    links,
    el("p", { class: "submission-meta" },
      el("span", { class: "submission-time", text: t("submittedAt", { time: record.created_at }) }),
      el("span", { class: "submission-source", text: t("submissionSource", { source: t(SOURCE_LABEL_KEY[record.source] ?? "sourceUnknown") }) }),
    ),
  );
  if (honor) panel.prepend(el("p", {
    class: "notice",
    text: t("honorUaNotice"),
  }));
  row.append(el("td", { colspan: "5" }, panel));
  return row;
}

function rowPair(record) {
  const expanded = openDetails.has(record.id);
  const version = displayVersion(record);
  const tr = el("tr", { id: `r${record.id}`, class: record.id === lastSavedId ? "record-row saved-row" : "record-row" });
  const deviceChildren = [el("div", { class: "device-name", text: record.dev })];
  if (record.deviceName) deviceChildren.push(el("div", { class: "device-sub", text: record.deviceName }));
  tr.append(cell(t("colDevice"), "device-cell", ...deviceChildren));
  tr.append(cell("VENDOR", "vendor-cell", el("span", { class: "vendor-value", text: record.vendor })));
  tr.append(cell(t("colVersion"), "target-cell",
    el("div", { class: "target-version", text: version }),
  ));
  const actionLabel = t(record.upgrade_or_erec === "emulater" ? EMULATER_LABEL_KEY[record.vendor] : ACTION_LABEL_KEY[record.upgrade_or_erec]);
  tr.append(cell(t("packageType"), "type-cell", el("span", {
    class: `badge badge-${record.upgrade_or_erec}`, text: actionLabel,
  })));
  const button = el("button", {
    type: "button", class: "button button-secondary button-small details-toggle",
    "aria-expanded": String(expanded), "aria-controls": `details-${record.id}`,
    "aria-label": t("toggleDetails", { action: t(expanded ? "collapse" : "expand"), dev: record.dev, version }),
    text: t(expanded ? "collapse" : "expand"),
  });
  button.addEventListener("click", () => {
    if (openDetails.has(record.id)) openDetails.delete(record.id);
    else openDetails.add(record.id);
    renderRecords();
  });
  tr.append(cell(t("colDetails"), "operations-cell", button));
  const fragment = document.createDocumentFragment();
  fragment.append(tr, detailsRow(record));
  return fragment;
}

function emptyRow(title, description) {
  return el("tr", {}, el("td", { colspan: "5", class: "empty-state" },
    el("strong", { text: title }), el("p", { text: description }),
  ));
}

function renderRecords() {
  if (!hasLoaded) return;
  $totalCount.textContent = String(totalCount);
  const fragment = document.createDocumentFragment();
  if (!records.length) {
    const filtering = Boolean($search.value.trim() || $actionFilter.value || $showEmulators.checked);
    fragment.append(filtering
      ? emptyRow(t("noMatchTitle"), t("noMatchDescription"))
      : emptyRow(t("emptyTitle"), t("emptyDescription")));
  } else {
    for (const record of records) fragment.append(rowPair(record));
  }
  $rows.replaceChildren(fragment);
  renderPagination();
}

function goToPage(page) {
  if (page < 1 || page > totalPages || page === currentPage) return;
  document.getElementById("records-panel").scrollIntoView({ block: "start" });
  refresh(page);
}

function renderPagination() {
  const $pagination = document.getElementById("pagination");
  if (totalPages <= 1) {
    $pagination.hidden = true;
    $pagination.replaceChildren();
    return;
  }
  const wanted = [1, totalPages, currentPage - 1, currentPage, currentPage + 1]
    .filter(n => n >= 1 && n <= totalPages);
  const pages = [...new Set(wanted)].sort((a, b) => a - b);

  const info = el("span", { class: "page-info", text: t("pageInfo", { page: currentPage, total: totalPages }) });
  const prev = el("button", { type: "button", class: "page-btn", text: t("previousPage") });
  const next = el("button", { type: "button", class: "page-btn", text: t("nextPage") });
  prev.disabled = currentPage === 1;
  next.disabled = currentPage === totalPages;
  prev.addEventListener("click", () => goToPage(currentPage - 1));
  next.addEventListener("click", () => goToPage(currentPage + 1));

  const numbers = el("span", { class: "page-numbers" });
  let previous = 0;
  for (const n of pages) {
    if (n - previous > 1) numbers.append(el("span", { class: "page-ellipsis", text: "…" }));
    const btn = el("button", { type: "button", class: "page-btn", text: String(n) });
    if (n === currentPage) {
      btn.classList.add("page-active");
      btn.setAttribute("aria-current", "page");
      btn.disabled = true;
    } else {
      btn.addEventListener("click", () => goToPage(n));
    }
    numbers.append(btn);
    previous = n;
  }

  $pagination.hidden = false;
  $pagination.replaceChildren(info, prev, numbers, next);
}

function buildQuery(page) {
  const params = new URLSearchParams();
  params.set("page", String(page));
  const search = $search.value.trim();
  if (search) params.set("q", search);
  if ($actionFilter.value) params.set("action", $actionFilter.value);
  if ($showEmulators.checked) params.set("include_emulators", "1");
  return `/requests?${params.toString()}`;
}

async function refresh(page = currentPage) {
  const version = ++loadVersion;
  $recordList.setAttribute("aria-busy", "true");
  try {
    const data = await requestJson(buildQuery(page));
    if (version !== loadVersion) return;
    if (!Array.isArray(data.requests) || data.requests.some(record => !Array.isArray(record.packages) || !record.packages.length)) {
      throw localizedError("invalidList");
    }
    const perPage = Number(data.perPage) || PER_PAGE;
    totalCount = Number(data.total) || 0;
    totalPages = totalCount ? Math.max(1, Math.ceil(totalCount / perPage)) : 1;
    if (page > totalPages) return refresh(totalPages);
    currentPage = page;
    records = data.requests;
    hasLoaded = true;
    loadErrored = false;
    renderRecords();
  } catch (error) {
    if (version !== loadVersion) return;
    showMsg(() => t("loadFailed", { error: errorText(error) }), true);
    if (!hasLoaded) {
      loadErrored = true;
      renderLoadError();
    }
  } finally {
    if (version === loadVersion) $recordList.setAttribute("aria-busy", "false");
  }
}

function renderLoadError() {
  const row = emptyRow(t("loadErrorTitle"), t("loadErrorDescription"));
  const retry = el("button", { type: "button", class: "button button-secondary button-small", text: t("reload") });
  retry.addEventListener("click", () => refresh());
  row.firstElementChild.append(retry);
  $rows.replaceChildren(row);
}

function bindSubmission(form, button, path, buildPayload) {
  let submitting = false;
  form.addEventListener("submit", async event => {
    event.preventDefault();
    if (submitting || !form.reportValidity()) return;
    let payload;
    try {
      payload = buildPayload();
    } catch (error) {
      showMsg(() => errorText(error), true);
      return;
    }
    submitting = true;
    activeSubmissions += 1;
    $brand.disabled = true;
    button.disabled = true;
    button.dataset.i18n = "verifying";
    button.textContent = t("verifying");
    form.setAttribute("aria-busy", "true");
    const brand = payload.brand;
    showMsg(path === "/requests/manual"
      ? () => t("verifyingManual")
      : () => t("verifyingBrand", { brand: t(brand === "honor" ? "honor" : "huawei") }));
    try {
      const saved = await requestJson(path, {
        method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(payload),
      });
      lastSavedId = saved.id;
      $search.value = "";
      $actionFilter.value = "";
      $showEmulators.checked = false;
      showMsg(() => t("saved"));
      await refresh(1);
    } catch (error) {
      showMsg(() => errorText(error), true);
    } finally {
      submitting = false;
      activeSubmissions -= 1;
      $brand.disabled = activeSubmissions > 0;
      button.disabled = false;
      button.dataset.i18n = "submit";
      button.textContent = t("submit");
      form.setAttribute("aria-busy", "false");
    }
  });
}

bindSubmission($form, $submitBtn, "/requests", () => {
  const payload = Object.fromEntries(new FormData($form).entries());
  payload.brand = $brand.value;
  if (!$patchEnabled.checked) delete payload.patch;
  if (payload.brand === "huawei" && $erecoveryEnabled.checked) payload.erecovery = true;
  return payload;
});

bindSubmission($manualForm, $manualSubmitBtn, "/requests/manual", () => {
  const urls = $manualUrls.value.split(/\r?\n/).map(line => line.trim()).filter(Boolean);
  if (urls.length !== 3 && urls.length !== 4) {
    throw localizedError("manualCount");
  }
  if (urls.some(url => !url.startsWith("http://update.dbankcdn.com/"))) {
    throw localizedError("manualPrefix");
  }
  return { urls };
});


$vendor.addEventListener("input", () => {
  vendorBeforeAutoParse = null;
});
$autoParse.addEventListener("input", () => {
  const raw = $autoParse.value.trim().toUpperCase();
  $autoParse.classList.remove("parse-ok", "parse-bad");
  if (!raw) return;
  const match = AUTO_PARSE_RE.exec(raw);
  if (!match) {
    $autoParse.classList.add("parse-bad");
    return;
  }
  // P 位 = PRELOAD 版本末位，R 位只进括号（真机证据：TAS 104.2.0.136(C00E136R7P4) → PRELOAD 104.0.0.4(C00R7)）。
  const [, series, model, lgrp, version, sp, region, custE, preloadR, preloadP, patch] = match;
  const regionRule = REGION_RULES[region];
  if (regionRule) {
    vendorBeforeAutoParse ??= $vendor.value;
    $vendor.value = regionRule.vendor;
  } else if (vendorBeforeAutoParse !== null) {
    $vendor.value = vendorBeforeAutoParse;
    vendorBeforeAutoParse = null;
  }
  const prefix = version.split(".").slice(0, -1).join(".");
  document.getElementById("dev").value = `${series}-${model}`;
  document.getElementById("base").value = `${series}-${lgrp}-${regionRule?.baseSuffix ?? "CHN"} ${version}${sp ? `(SP${sp})` : ""}`;
  if (patch !== undefined) $patch.value = `patch${patch}`;
  document.getElementById("cust").value = `${series}-${model}-CUST ${prefix}.${custE}(${region})`;
  document.getElementById("preload").value = `${series}-${model}-PRELOAD ${prefix}.${preloadP}(${region}R${preloadR})`;

  $autoParse.classList.add("parse-ok");
});
$patchEnabled.addEventListener("change", () => {
  $patch.disabled = !$patchEnabled.checked;
});
$brand.addEventListener("change", () => {
  patchByBrand[selectedBrand] = $patchEnabled.checked;
  selectedBrand = $brand.value;
  const honor = selectedBrand === "honor";
  for (const [id, placeholder] of Object.entries(brandPlaceholders[selectedBrand])) {
    document.getElementById(id).placeholder = placeholder;
  }
  $patchEnabled.checked = patchByBrand[selectedBrand];
  $patch.disabled = !$patchEnabled.checked;
  document.getElementById("manual-method").hidden = honor;
  $erecoveryEnabled.closest("label").hidden = honor;
  document.getElementById("version-method").open = true;
  $msg.hidden = true;
});
$search.addEventListener("input", () => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => refresh(1), 300);
});
$actionFilter.addEventListener("change", () => refresh(1));
$showEmulators.addEventListener("change", () => refresh(1));
$themeToggle.addEventListener("click", () => {
  applyTheme(document.documentElement.dataset.theme === "light" ? "dark" : "light");
});
function applyTheme(theme) {
  document.documentElement.dataset.theme = theme;
  $themeToggle.setAttribute("aria-pressed", String(theme === "dark"));
  document.querySelector('meta[name="theme-color"]').setAttribute("content", THEME_COLORS[theme]);
  try { localStorage.setItem("theme", theme); } catch (e) {}
}
applyTheme(document.documentElement.dataset.theme || "dark");
$langSelect.addEventListener("change", () => {
  setLanguage($langSelect.value, true);
  if (hasLoaded) renderRecords();
  else if (loadErrored) renderLoadError();
  if (!$msg.hidden) paintMsg();
});
refresh(1);
