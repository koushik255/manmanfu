<script lang="ts">
  import { convertFileSrc, invoke, isTauri } from "@tauri-apps/api/core";
  import { onMount } from "svelte";

  const mangaLibraryRoot = "/home/koushikk/MANGA";

  type MangaSeries = {
    name: string;
    path: string;
  };

  type VolumeSummary = {
    name: string;
    path: string;
  };

  type OpenedVolume = {
    title: string;
    path: string;
    pageCount: number;
  };

  type PageImage = {
    imagePath: string;
    pageIndex: number;
    pageCount: number;
  };

  type CachedPage = {
    imageSrc: string;
    pageIndex: number;
    pageCount: number;
    width: number;
    height: number;
  };

  type Bookmark = {
    id: string;
    createdAt: string;
    seriesPath: string;
    seriesName: string;
    volumePath: string;
    volumeName: string;
    /** Index into the volume for the current view (`loadView` start), 0-based */
    viewStart: number;
  };

  let mangaSeries: MangaSeries[] = [];
  let selectedSeriesPath = "";
  let volumes: VolumeSummary[] = [];
  let selectedVolumePath = "";
  let currentVolume: OpenedVolume | null = null;
  let currentViewStart = 0;
  let currentViewPages: CachedPage[] = [];
  let loadingVolumes = true;
  let loadingPage = false;
  let errorMessage = "";
  let pageCache: Record<number, CachedPage> = {};
  let activeViewRequest = 0;
  let sidebarCollapsed = false;
  let settingsOpen = false;
  let exportDir = "";
  let saveFeedback = "";
  let settingsWrapEl: HTMLDivElement | undefined = undefined;
  let bookmarksWrapEl: HTMLDivElement | undefined = undefined;
  let saveFeedbackClear: ReturnType<typeof setTimeout> | undefined = undefined;
  let bookmarksMenuOpen = false;
  let bookmarks: Bookmark[] = [];
  let bedReaderMode = false;
  /** Sideways rotation: clockwise +90° vs counter-clockwise −90° */
  let bedRotation: "cw" | "ccw" = "cw";

  const EXPORT_DIR_STORAGE_KEY = "mr_export_dir";
  const BED_READER_STORAGE_KEY = "mr_bed_reader";
  const BED_ROTATION_STORAGE_KEY = "mr_bed_rotation";
  const BOOKMARKS_STORAGE_KEY = "mr_bookmarks";

  function loadBookmarksFromStorage(): Bookmark[] {
    if (typeof localStorage === "undefined") {
      return [];
    }

    try {
      const raw = localStorage.getItem(BOOKMARKS_STORAGE_KEY);
      if (!raw) {
        return [];
      }

      const parsed = JSON.parse(raw) as unknown;
      if (!Array.isArray(parsed)) {
        return [];
      }

      return parsed.filter((entry): entry is Bookmark => {
        if (!entry || typeof entry !== "object") {
          return false;
        }

        const bookmark = entry as Record<string, unknown>;
        return (
          typeof bookmark.id === "string" &&
          typeof bookmark.createdAt === "string" &&
          typeof bookmark.seriesPath === "string" &&
          typeof bookmark.seriesName === "string" &&
          typeof bookmark.volumePath === "string" &&
          typeof bookmark.volumeName === "string" &&
          typeof bookmark.viewStart === "number" &&
          Number.isFinite(bookmark.viewStart)
        );
      });
    } catch {
      return [];
    }
  }

  function persistBookmarks() {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(BOOKMARKS_STORAGE_KEY, JSON.stringify(bookmarks));
    }
  }

  function removeBookmark(bookmarkId: string) {
    bookmarks = bookmarks.filter((bookmark) => bookmark.id !== bookmarkId);
    persistBookmarks();
  }

  function createBookmark() {
    if (!currentVolume || !selectedSeriesPath || !selectedVolumePath) {
      showSaveFeedback("Open a volume to bookmark.");
      return;
    }

    const seriesName = mangaSeries.find((series) => series.path === selectedSeriesPath)?.name ?? "Series";
    const volumeName =
      volumes.find((volume) => volume.path === selectedVolumePath)?.name.replace(/\.cbz$/i, "") ?? "Volume";

    const bookmark: Bookmark = {
      id: crypto.randomUUID(),
      createdAt: new Date().toISOString(),
      seriesPath: selectedSeriesPath,
      seriesName,
      volumePath: selectedVolumePath,
      volumeName,
      viewStart: currentViewStart,
    };

    bookmarks = [bookmark, ...bookmarks];
    persistBookmarks();
    showSaveFeedback("Bookmark saved.");
  }

  async function goToBookmark(bookmark: Bookmark) {
    bookmarksMenuOpen = false;
    errorMessage = "";

    try {
      if (selectedSeriesPath !== bookmark.seriesPath) {
        loadingVolumes = true;
        selectedSeriesPath = bookmark.seriesPath;
        selectedVolumePath = "";
        currentVolume = null;
        currentViewPages = [];
        pageCache = {};
        activeViewRequest += 1;
        volumes = await invoke<VolumeSummary[]>("list_volumes", { rootPath: bookmark.seriesPath });

        if (!volumes.some((volume) => volume.path === bookmark.volumePath)) {
          errorMessage = "That bookmark’s volume is no longer in this series.";
          return;
        }
      } else if (!volumes.some((volume) => volume.path === bookmark.volumePath)) {
        loadingVolumes = true;
        volumes = await invoke<VolumeSummary[]>("list_volumes", { rootPath: bookmark.seriesPath });

        if (!volumes.some((volume) => volume.path === bookmark.volumePath)) {
          errorMessage = "That bookmark’s volume is no longer available.";
          return;
        }
      }

      await selectVolume(bookmark.volumePath, bookmark.viewStart);
    } catch (error) {
      errorMessage = String(error);
    } finally {
      loadingVolumes = false;
    }
  }

  function formatBookmarkWhen(iso: string) {
    try {
      return new Date(iso).toLocaleString(undefined, {
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      });
    } catch {
      return "";
    }
  }

  function isEditableTarget(target: EventTarget | null) {
    if (!(target instanceof HTMLElement)) {
      return false;
    }

    const tag = target.tagName;
    return (
      tag === "INPUT" ||
      tag === "TEXTAREA" ||
      tag === "SELECT" ||
      target.isContentEditable
    );
  }

  function showSaveFeedback(message: string) {
    saveFeedback = message;
    if (saveFeedbackClear !== undefined) {
      clearTimeout(saveFeedbackClear);
    }

    saveFeedbackClear = setTimeout(() => {
      saveFeedback = "";
      saveFeedbackClear = undefined;
    }, 4500);
  }

  function handleWindowClick(event: MouseEvent) {
    const node = event.target;
    if (!(node instanceof Node)) {
      return;
    }

    if (settingsOpen && settingsWrapEl && !settingsWrapEl.contains(node)) {
      settingsOpen = false;
    }

    if (bookmarksMenuOpen && bookmarksWrapEl && !bookmarksWrapEl.contains(node)) {
      bookmarksMenuOpen = false;
    }
  }

  function persistExportDir() {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(EXPORT_DIR_STORAGE_KEY, exportDir);
    }
  }

  function persistBedReaderMode() {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(BED_READER_STORAGE_KEY, bedReaderMode ? "1" : "0");
    }
  }

  async function applyBedReaderModeChange() {
    persistBedReaderMode();
    if (currentVolume && !loadingPage) {
      await loadView(currentViewStart);
    }
  }

  function persistBedRotation() {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(BED_ROTATION_STORAGE_KEY, bedRotation);
    }
  }

  function getPageCounterText(): string {
    if (!currentVolume) {
      return "";
    }

    const total = currentVolume.pageCount;

    if (currentViewPages.length === 0) {
      return loadingPage ? `— / ${total}` : "";
    }

    const pageNumbers = currentViewPages.map((page) => page.pageIndex + 1).sort((a, b) => a - b);
    const low = pageNumbers[0];
    const high = pageNumbers[pageNumbers.length - 1];

    if (pageNumbers.length === 1) {
      const stepHint = bedReaderMode
        ? "Next/Previous moves 1 page (bed mode)"
        : "Next/Previous moves 1 page (wide or cover)";
      return `Page ${low} of ${total} · ${stepHint}`;
    }

    return `Pages ${low}–${high} of ${total} · Next/Previous moves 2 pages (spread)`;
  }

  async function saveLeftPage() {
    if (!isTauri()) {
      showSaveFeedback("Saving pages requires the Tauri desktop app.");
      return;
    }

    const left = currentViewPages[0];
    if (!left) {
      showSaveFeedback("No page is shown on the left.");
      return;
    }

    const dir = exportDir.trim();
    if (!dir) {
      showSaveFeedback("Choose an export folder in Settings first.");
      return;
    }

    try {
      const savedPath = await invoke<string>("save_page_image_export", {
        pageIndex: left.pageIndex,
        destinationDir: dir,
      });
      showSaveFeedback(`Saved left page to ${savedPath}`);
    } catch (error) {
      showSaveFeedback(String(error));
    }
  }

  async function saveRightPage() {
    if (!isTauri()) {
      showSaveFeedback("Saving pages requires the Tauri desktop app.");
      return;
    }

    const right = currentViewPages[1];
    if (!right) {
      showSaveFeedback("No right page in this spread (single page or wide page).");
      return;
    }

    const dir = exportDir.trim();
    if (!dir) {
      showSaveFeedback("Choose an export folder in Settings first.");
      return;
    }

    try {
      const savedPath = await invoke<string>("save_page_image_export", {
        pageIndex: right.pageIndex,
        destinationDir: dir,
      });
      showSaveFeedback(`Saved right page to ${savedPath}`);
    } catch (error) {
      showSaveFeedback(String(error));
    }
  }

  async function loadLibrary() {
    loadingVolumes = true;
    errorMessage = "";

    try {
      mangaSeries = await invoke<MangaSeries[]>("list_manga_series", { rootPath: mangaLibraryRoot });
      if (mangaSeries.length > 0) {
        await selectSeries(mangaSeries[0].path);
      } else {
        errorMessage = "No manga folders with .cbz volumes were found.";
      }
    } catch (error) {
      errorMessage = String(error);
    } finally {
      loadingVolumes = false;
    }
  }

  async function selectSeries(seriesPath: string) {
    if (!seriesPath) {
      return;
    }

    loadingVolumes = true;
    errorMessage = "";

    try {
      selectedSeriesPath = seriesPath;
      selectedVolumePath = "";
      currentVolume = null;
      currentViewPages = [];
      pageCache = {};
      volumes = await invoke<VolumeSummary[]>("list_volumes", { rootPath: seriesPath });

      if (volumes.length > 0) {
        await selectVolume(volumes[0].path);
      } else {
        errorMessage = "This manga folder does not contain any .cbz volumes.";
      }
    } catch (error) {
      errorMessage = String(error);
    } finally {
      loadingVolumes = false;
    }
  }

  async function selectVolume(volumePath: string, initialViewStart = 0) {
    if (!volumePath) {
      return;
    }

    /* Use `currentVolume.path`, not `selectedVolumePath`: the select’s bind:value
     * updates before onchange, so the bound path can already match the new volume
     * while `currentVolume` still refers to the previous file. */
    if (currentVolume && currentVolume.path === volumePath) {
      const clamped = Math.min(
        Math.max(0, initialViewStart),
        Math.max(0, currentVolume.pageCount - 1),
      );
      await loadView(clamped);
      return;
    }

    loadingPage = true;
    errorMessage = "";

    try {
      activeViewRequest += 1;
      selectedVolumePath = volumePath;
      currentViewPages = [];
      pageCache = {};
      currentVolume = await invoke<OpenedVolume>("open_volume", { volumePath });
      const start = Math.min(
        Math.max(0, initialViewStart),
        Math.max(0, currentVolume.pageCount - 1),
      );
      await loadView(start);
    } catch (error) {
      errorMessage = String(error);
      currentVolume = null;
      currentViewPages = [];
    } finally {
      loadingPage = false;
    }
  }

  function measureImage(imageSrc: string) {
    return new Promise<{ width: number; height: number }>((resolve, reject) => {
      const image = new Image();
      image.onload = () => resolve({ width: image.naturalWidth, height: image.naturalHeight });
      image.onerror = () => reject(new Error("failed to measure manga page dimensions"));
      image.src = imageSrc;
    });
  }

  async function ensurePage(pageIndex: number) {
    const cachedPage = pageCache[pageIndex];
    if (cachedPage) {
      return cachedPage;
    }

    const page = await invoke<PageImage>("get_page_image", { pageIndex });
    const imageSrc = isTauri() ? convertFileSrc(page.imagePath) : page.imagePath;
    const { width, height } = await measureImage(imageSrc);
    const loadedPage: CachedPage = {
      imageSrc,
      pageIndex: page.pageIndex,
      pageCount: page.pageCount,
      width,
      height,
    };

    pageCache = {
      ...pageCache,
      [pageIndex]: loadedPage,
    };

    return loadedPage;
  }

  function shouldDisplaySingle(page: CachedPage) {
    return page.pageIndex === 0 || page.width > page.height;
  }

  async function getViewPages(startIndex: number) {
    if (!currentVolume || startIndex < 0 || startIndex >= currentVolume.pageCount) {
      return [];
    }

    if (bedReaderMode) {
      const page = await ensurePage(startIndex);
      return [page];
    }

    const firstPage = await ensurePage(startIndex);
    if (shouldDisplaySingle(firstPage)) {
      return [firstPage];
    }

    const secondIndex = startIndex + 1;
    if (secondIndex >= currentVolume.pageCount) {
      return [firstPage];
    }

    const secondPage = await ensurePage(secondIndex);
    if (shouldDisplaySingle(secondPage)) {
      return [firstPage];
    }

    return [secondPage, firstPage];
  }

  async function loadView(startIndex: number) {
    if (!currentVolume) {
      return;
    }

    const requestId = ++activeViewRequest;
    loadingPage = true;
    errorMessage = "";

    try {
      const pages = await getViewPages(startIndex);
      if (requestId !== activeViewRequest) {
        return;
      }

      currentViewStart = startIndex;
      currentViewPages = pages;
    } catch (error) {
      if (requestId !== activeViewRequest) {
        return;
      }

      errorMessage = String(error);
    } finally {
      if (requestId === activeViewRequest) {
        loadingPage = false;
      }
    }
  }

  async function getPreviousViewStart() {
    if (!currentVolume || currentViewStart === 0) {
      return 0;
    }

    let previousStart = 0;
    let probeIndex = 0;

    while (probeIndex < currentViewStart) {
      previousStart = probeIndex;
      const pages = await getViewPages(probeIndex);
      probeIndex += Math.max(pages.length, 1);
    }

    return previousStart;
  }

  async function showPreviousPage() {
    if (!currentVolume || currentViewStart === 0 || loadingPage) {
      return;
    }

    const previousStart = await getPreviousViewStart();
    await loadView(previousStart);
  }

  async function showNextPage() {
    if (!currentVolume || loadingPage || currentViewPages.length === 0) {
      return;
    }

    const nextStart = currentViewStart + currentViewPages.length;
    if (nextStart >= currentVolume.pageCount) {
      return;
    }

    await loadView(nextStart);
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      if (bookmarksMenuOpen) {
        event.preventDefault();
        bookmarksMenuOpen = false;
        return;
      }

      if (settingsOpen) {
        event.preventDefault();
        settingsOpen = false;
        return;
      }
    }

    if (isEditableTarget(event.target)) {
      return;
    }

    if ((event.key === "m" || event.key === "M") && !event.ctrlKey && !event.metaKey && !event.altKey) {
      event.preventDefault();
      createBookmark();
      return;
    }

    if (event.key === "ArrowLeft") {
      event.preventDefault();
      void showPreviousPage();
      return;
    }

    if (event.key === "ArrowRight") {
      event.preventDefault();
      void showNextPage();
      return;
    }

    if ((event.key === "s" || event.key === "S") && !event.ctrlKey && !event.metaKey && !event.altKey) {
      event.preventDefault();
      void saveLeftPage();
      return;
    }

    if ((event.key === "d" || event.key === "D") && !event.ctrlKey && !event.metaKey && !event.altKey) {
      event.preventDefault();
      void saveRightPage();
    }
  }

  onMount(() => {
    if (typeof localStorage !== "undefined") {
      bookmarks = loadBookmarksFromStorage();

      const stored = localStorage.getItem(EXPORT_DIR_STORAGE_KEY);
      if (stored) {
        exportDir = stored;
      }

      const bedStored = localStorage.getItem(BED_READER_STORAGE_KEY);
      if (bedStored !== null) {
        bedReaderMode = bedStored === "1";
      }

      const rotationStored = localStorage.getItem(BED_ROTATION_STORAGE_KEY);
      if (rotationStored === "cw" || rotationStored === "ccw") {
        bedRotation = rotationStored;
      }
    }

    void loadLibrary();

    if (isTauri() && !exportDir.trim()) {
      void invoke<string>("default_export_directory")
        .then((path) => {
          exportDir = path;
        })
        .catch(() => {
          /* keep empty until user sets path */
        });
    }
  });
</script>

<svelte:head>
  <title>mr</title>
</svelte:head>

<svelte:window onkeydown={handleKeydown} onclick={handleWindowClick} />

<div class:sidebar-collapsed={sidebarCollapsed} class="app-shell">
  <aside class="sidebar" aria-label="Reader width">
    <button
      class="sidebar-toggle"
      type="button"
      onclick={() => {
        sidebarCollapsed = !sidebarCollapsed;
      }}
      aria-expanded={!sidebarCollapsed}
      aria-label={sidebarCollapsed ? "Expand sidebar" : "Collapse sidebar"}
    >
      {sidebarCollapsed ? "»" : "«"}
    </button>
  </aside>

  <main class="reader" class:bed-reader-mode={bedReaderMode}>
    <header class="reader-header">
      <div class="reader-header-library">
        <div class="header-select-group">
          <label class="header-select-label" for="series-select">Series</label>
          <select
            id="series-select"
            class="series-select header-select"
            bind:value={selectedSeriesPath}
            onchange={(event) => void selectSeries((event.currentTarget as HTMLSelectElement).value)}
          >
            {#each mangaSeries as series}
              <option value={series.path}>{series.name}</option>
            {/each}
          </select>
        </div>
        <div class="header-select-group">
          <label class="header-select-label" for="volume-select">Volume</label>
          <select
            id="volume-select"
            class="series-select header-select"
            bind:value={selectedVolumePath}
            disabled={loadingVolumes || volumes.length === 0}
            onchange={(event) => void selectVolume((event.currentTarget as HTMLSelectElement).value)}
          >
            {#if loadingVolumes}
              <option value="">Loading volumes…</option>
            {:else if volumes.length === 0}
              <option value="">No volumes</option>
            {:else}
              {#each volumes as volume}
                <option value={volume.path}>{volume.name.replace(".cbz", "")}</option>
              {/each}
            {/if}
          </select>
        </div>

        <div class="bookmarks-wrap" bind:this={bookmarksWrapEl}>
          <button
            class="bookmarks-toggle"
            type="button"
            aria-expanded={bookmarksMenuOpen}
            aria-haspopup="true"
            onclick={(event) => {
              event.stopPropagation();
              bookmarksMenuOpen = !bookmarksMenuOpen;
              if (bookmarksMenuOpen) {
                settingsOpen = false;
              }
            }}
          >
            Bookmarks
          </button>

          {#if bookmarksMenuOpen}
            <div class="bookmarks-panel" role="region" aria-label="Saved bookmarks">
              {#if bookmarks.length === 0}
                <p class="bookmarks-empty">
                  No bookmarks yet. Press <kbd class="bookmarks-kbd">M</kbd> to save the current page.
                </p>
              {:else}
                <ul class="bookmarks-list">
                  {#each bookmarks as bookmark (bookmark.id)}
                    <li class="bookmark-row">
                      <button
                        type="button"
                        class="bookmark-jump"
                        onclick={() => void goToBookmark(bookmark)}
                      >
                        <span class="bookmark-title">{bookmark.seriesName}</span>
                        <span class="bookmark-detail"
                          >{bookmark.volumeName} · page {bookmark.viewStart + 1} in volume</span
                        >
                        <span class="bookmark-when">{formatBookmarkWhen(bookmark.createdAt)}</span>
                      </button>
                      <button
                        type="button"
                        class="bookmark-remove"
                        aria-label="Remove bookmark"
                        onclick={() => removeBookmark(bookmark.id)}
                      >
                        ×
                      </button>
                    </li>
                  {/each}
                </ul>
              {/if}
            </div>
          {/if}
        </div>
      </div>

      <div class="reader-title-spacer"></div>

      {#if currentVolume}
        {@const pageCounterText = getPageCounterText()}
        {#if pageCounterText}
          <p class="reader-page-counter" aria-live="polite">{pageCounterText}</p>
        {/if}
      {/if}

      {#if saveFeedback}
        <p class="reader-feedback" role="status">{saveFeedback}</p>
      {/if}

      <div class="reader-header-actions">
        <div class="settings-wrap" bind:this={settingsWrapEl}>
          <button
            class="settings-toggle"
            type="button"
            aria-expanded={settingsOpen}
            aria-haspopup="true"
            onclick={(event) => {
              event.stopPropagation();
              settingsOpen = !settingsOpen;
              if (settingsOpen) {
                bookmarksMenuOpen = false;
              }
            }}
          >
            Settings
          </button>

          {#if settingsOpen}
            <div class="settings-panel" role="region" aria-label="Reader settings">
              <h2 class="settings-heading">Export folder</h2>
              <p class="settings-hint">Page images are copied here when you press <kbd>S</kbd> (left) or <kbd>D</kbd> (right).</p>
              <label class="settings-label" for="export-dir-input">Folder path</label>
              <input
                id="export-dir-input"
                class="settings-input"
                type="text"
                bind:value={exportDir}
                placeholder="/home/you/Pictures/mr-captures"
                autocomplete="off"
                onblur={() => persistExportDir()}
              />
              <button
                class="settings-save-dir"
                type="button"
                onclick={() => {
                  persistExportDir();
                  showSaveFeedback("Export folder saved.");
                }}
              >
                Save folder
              </button>

              <div class="settings-divider" role="presentation"></div>

              <h2 class="settings-heading">Sideways reading</h2>
              <p class="settings-hint">
                Show one page at a time and rotate it 90° so you can lay on your side with the device.
              </p>
              <label class="settings-checkbox-row" for="bed-reader-toggle">
                <input
                  id="bed-reader-toggle"
                  type="checkbox"
                  bind:checked={bedReaderMode}
                  onchange={() => void applyBedReaderModeChange()}
                />
                <span>Bed / sideways mode</span>
              </label>

              <fieldset class="settings-fieldset">
                <legend class="settings-legend">Sideways rotation</legend>
                <p class="settings-hint settings-hint-tight">Only applies while bed / sideways mode is on.</p>
                <label class="settings-radio-row" for="bed-rot-cw">
                  <input
                    id="bed-rot-cw"
                    type="radio"
                    name="bed-rotation"
                    value="cw"
                    bind:group={bedRotation}
                    onchange={() => persistBedRotation()}
                  />
                  <span>Clockwise (90°)</span>
                </label>
                <label class="settings-radio-row" for="bed-rot-ccw">
                  <input
                    id="bed-rot-ccw"
                    type="radio"
                    name="bed-rotation"
                    value="ccw"
                    bind:group={bedRotation}
                    onchange={() => persistBedRotation()}
                  />
                  <span>Counter-clockwise (−90°)</span>
                </label>
              </fieldset>
            </div>
          {/if}
        </div>

        <div class="controls">
          <button onclick={() => void showPreviousPage()} disabled={!currentVolume || currentViewStart === 0 || loadingPage}>
            Previous
          </button>
          <button
            onclick={() => void showNextPage()}
            disabled={!currentVolume || currentViewStart + currentViewPages.length >= (currentVolume?.pageCount ?? 0) || loadingPage}
          >
            Next
          </button>
        </div>
      </div>
    </header>

    {#if errorMessage}
      <p class="status error">{errorMessage}</p>
    {/if}

    <section class="page-stage" class:bed-mode={bedReaderMode}>
      {#if currentViewPages.length > 0}
        <div
          class:single-page={currentViewPages.length === 1 || bedReaderMode}
          class="spread"
        >
          {#each currentViewPages as page}
            {#if bedReaderMode}
              <div class="bed-spin">
                <img
                  class="page-image page-image-bed"
                  class:page-image-bed--cw={bedRotation === "cw"}
                  class:page-image-bed--ccw={bedRotation === "ccw"}
                  src={page.imageSrc}
                  alt={`Manga page ${page.pageIndex + 1}`}
                />
              </div>
            {:else}
              <img
                class="page-image"
                src={page.imageSrc}
                alt={`Manga page ${page.pageIndex + 1}`}
              />
            {/if}
          {/each}
        </div>
      {:else if loadingPage}
        <p class="status">Loading page...</p>
      {:else}
        <p class="status">No page loaded.</p>
      {/if}
    </section>
  </main>
</div>

<style>
  :global(html, body) {
    margin: 0;
    width: 100%;
    height: 100%;
    background: #0a0a0a;
    color: #ffffff;
    color-scheme: dark;
    font-family: "IBM Plex Sans", "Segoe UI", sans-serif;
  }

  :global(body) {
    min-height: 100vh;
  }

  .app-shell {
    display: grid;
    grid-template-columns: 280px 1fr;
    width: 100vw;
    height: 100vh;
    background:
      radial-gradient(circle at top, rgba(210, 184, 138, 0.08), transparent 30%),
      linear-gradient(180deg, #111 0%, #050505 100%);
  }

  .app-shell.sidebar-collapsed {
    grid-template-columns: 56px 1fr;
  }

  .sidebar {
    border-right: 1px solid rgba(255, 255, 255, 0.08);
    background: rgba(16, 16, 16, 0.92);
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    padding: 1rem 0.65rem;
    min-height: 0;
  }

  .app-shell.sidebar-collapsed .sidebar {
    align-items: center;
    padding-inline: 0.45rem;
  }

  .status {
    margin: 0.35rem 0 0;
    color: #ffffff;
  }

  .reader-header-library {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: 0.65rem 1rem;
    flex: 0 1 auto;
    min-width: 0;
  }

  .header-select-group {
    display: grid;
    gap: 0.2rem;
    min-width: 0;
    flex: 1 1 12.5rem;
    max-width: 26rem;
    color-scheme: light;
  }

  .header-select-label {
    color: #ffffff;
    font-size: 0.68rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    line-height: 1.2;
  }

  /* Light controls: document uses `color-scheme: dark`, which otherwise makes the
   * engine draw dark-themed native chrome (often a pale/white ring) around a
   * light `<select>`. Force light scheme on these controls only. */
  .series-select {
    width: 100%;
    min-width: 0;
    box-sizing: border-box;
    border: 1px solid rgba(0, 0, 0, 0.28);
    background: #f0f0f0;
    color: #0a0a0a;
    color-scheme: light;
    border-radius: 10px;
    padding: 0.75rem 0.85rem;
    font: inherit;
    box-shadow: none;
    outline: none;
  }

  .series-select:focus-visible {
    outline: 2px solid rgba(210, 184, 138, 0.85);
    outline-offset: 2px;
  }

  .reader-header-library .series-select {
    padding: 0.38rem 0.75rem;
    border-radius: 8px;
    font-size: 0.88rem;
    line-height: 1.25;
  }

  .series-select option {
    background: #ffffff;
    color: #0a0a0a;
    color-scheme: light;
  }

  .series-select:disabled {
    opacity: 0.75;
    cursor: not-allowed;
    color: #333333;
    background: #e0e0e0;
  }

  .header-select {
    min-width: min(100%, 11.5rem);
  }

  .bookmarks-wrap {
    position: relative;
    align-self: flex-end;
  }

  .bookmarks-toggle {
    padding: 0.38rem 0.85rem;
    min-height: calc(0.88rem * 1.25 + 0.76rem + 2px);
    font-size: 0.88rem;
    line-height: 1.25;
    border-radius: 8px;
  }

  .bookmarks-panel {
    position: absolute;
    top: calc(100% + 0.45rem);
    left: 0;
    width: min(22rem, calc(100vw - 2rem));
    max-height: min(70vh, 24rem);
    overflow-x: hidden;
    overflow-y: auto;
    padding: 0.65rem 0.7rem;
    border-radius: 12px;
    border: 1px solid rgba(255, 255, 255, 0.12);
    background: #121212;
    box-shadow: 0 18px 48px rgba(0, 0, 0, 0.55);
    z-index: 21;
  }

  .bookmarks-empty {
    margin: 0;
    padding: 0.35rem 0.25rem;
    font-size: 0.82rem;
    line-height: 1.45;
    color: #ffffff;
  }

  .bookmarks-kbd {
    display: inline-block;
    padding: 0.08rem 0.35rem;
    border-radius: 4px;
    border: 1px solid rgba(255, 255, 255, 0.25);
    background: #1e1e1e;
    font-size: 0.78rem;
    font-family: inherit;
  }

  .bookmarks-list {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }

  .bookmark-row {
    display: flex;
    gap: 0.25rem;
    align-items: stretch;
    min-width: 0;
  }

  .bookmark-jump {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 0.15rem;
    padding: 0.45rem 0.55rem;
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.1);
    background: rgba(255, 255, 255, 0.03);
    color: #ffffff;
    font: inherit;
    text-align: left;
    cursor: pointer;
    transition:
      background 0.15s ease,
      border-color 0.15s ease;
  }

  .bookmark-jump:hover {
    background: rgba(210, 184, 138, 0.12);
    border-color: rgba(210, 184, 138, 0.35);
  }

  .bookmark-title {
    font-weight: 600;
    font-size: 0.88rem;
    line-height: 1.25;
  }

  .bookmark-detail {
    font-size: 0.78rem;
    line-height: 1.25;
    color: rgba(255, 255, 255, 0.88);
    word-break: break-word;
  }

  .bookmark-when {
    font-size: 0.72rem;
    color: rgba(255, 255, 255, 0.55);
  }

  .bookmark-remove {
    flex: 0 0 2rem;
    width: 2rem;
    padding: 0;
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.12);
    background: rgba(255, 255, 255, 0.04);
    color: #ffffff;
    font-size: 1.15rem;
    line-height: 1;
    cursor: pointer;
    transition:
      background 0.15s ease,
      border-color 0.15s ease;
  }

  .bookmark-remove:hover {
    background: rgba(220, 80, 80, 0.2);
    border-color: rgba(255, 120, 120, 0.45);
  }

  .controls button,
  .sidebar-toggle,
  .settings-toggle,
  .settings-save-dir,
  .bookmarks-toggle {
    border: 1px solid rgba(255, 255, 255, 0.1);
    background: rgba(255, 255, 255, 0.04);
    color: #ffffff;
    border-radius: 10px;
    cursor: pointer;
    transition:
      background 0.18s ease,
      border-color 0.18s ease,
      transform 0.18s ease;
  }

  .sidebar-toggle {
    width: 36px;
    height: 36px;
    flex: 0 0 auto;
    font-size: 1rem;
    display: inline-grid;
    place-items: center;
  }

  .controls button:hover:not(:disabled),
  .settings-toggle:hover,
  .settings-save-dir:hover,
  .bookmarks-toggle:hover {
    background: rgba(210, 184, 138, 0.14);
    border-color: rgba(210, 184, 138, 0.4);
    transform: translateY(-1px);
  }

  .reader {
    display: grid;
    grid-template-rows: auto 1fr;
    min-width: 0;
    min-height: 0;
  }

  .reader-header {
    display: flex;
    justify-content: flex-start;
    gap: 1rem;
    align-items: flex-start;
    padding: 1rem 1.25rem;
    border-bottom: 1px solid rgba(255, 255, 255, 0.08);
    background: rgba(10, 10, 10, 0.72);
  }

  .reader-title-spacer {
    min-height: 1px;
    flex: 1 1 auto;
  }

  .reader-page-counter {
    margin: 0.35rem 0 0;
    flex: 0 1 22rem;
    max-width: min(100%, 26rem);
    font-size: 0.88rem;
    line-height: 1.35;
    color: #ffffff;
    font-variant-numeric: tabular-nums;
  }

  .reader-feedback {
    margin: 0.35rem 0 0;
    flex: 1 1 12rem;
    max-width: min(100%, 28rem);
    font-size: 0.88rem;
    line-height: 1.35;
    color: #ffffff;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .reader-header-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-start;
    justify-content: flex-end;
    gap: 0.75rem;
  }

  .settings-wrap {
    position: relative;
  }

  .settings-toggle {
    padding: 0.8rem 1rem;
    min-width: 6.5rem;
  }

  .settings-panel {
    position: absolute;
    top: calc(100% + 0.5rem);
    right: 0;
    width: min(22rem, calc(100vw - 2.5rem));
    padding: 1rem 1.1rem;
    border-radius: 12px;
    border: 1px solid rgba(255, 255, 255, 0.12);
    background: #121212;
    box-shadow: 0 18px 48px rgba(0, 0, 0, 0.55);
    z-index: 20;
  }

  .settings-heading {
    margin: 0 0 0.5rem;
    font-size: 1rem;
    font-weight: 600;
    color: #ffffff;
  }

  .settings-hint {
    margin: 0 0 0.85rem;
    font-size: 0.82rem;
    line-height: 1.45;
    color: #ffffff;
  }

  .settings-hint-tight {
    margin-top: -0.25rem;
    margin-bottom: 0.55rem;
  }

  .settings-fieldset {
    margin: 0.85rem 0 0;
    padding: 0;
    border: none;
    min-width: 0;
  }

  .settings-legend {
    padding: 0;
    font-size: 0.78rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: #ffffff;
  }

  .settings-radio-row {
    display: flex;
    align-items: flex-start;
    gap: 0.55rem;
    margin-top: 0.45rem;
    cursor: pointer;
    font-size: 0.88rem;
    line-height: 1.4;
    color: #ffffff;
  }

  .settings-radio-row input {
    width: 1.05rem;
    height: 1.05rem;
    margin-top: 0.14rem;
    flex-shrink: 0;
    accent-color: #d2b88a;
  }

  .settings-label {
    display: block;
    margin-bottom: 0.35rem;
    font-size: 0.78rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: #ffffff;
  }

  .settings-input {
    width: 100%;
    box-sizing: border-box;
    margin-bottom: 0.75rem;
    padding: 0.55rem 0.65rem;
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.2);
    background: #0d0d0d;
    color: #ffffff;
    font: inherit;
    font-size: 0.88rem;
  }

  .settings-input::placeholder {
    color: rgba(255, 255, 255, 0.45);
  }

  .settings-save-dir {
    width: 100%;
    padding: 0.65rem 0.85rem;
    font-size: 0.92rem;
  }

  .settings-divider {
    height: 1px;
    margin: 1rem 0;
    background: rgba(255, 255, 255, 0.12);
  }

  .settings-checkbox-row {
    display: flex;
    align-items: flex-start;
    gap: 0.65rem;
    cursor: pointer;
    font-size: 0.92rem;
    line-height: 1.45;
    color: #ffffff;
  }

  .settings-checkbox-row input {
    width: 1.1rem;
    height: 1.1rem;
    margin-top: 0.12rem;
    flex-shrink: 0;
    accent-color: #d2b88a;
  }

  .settings-panel kbd {
    display: inline-block;
    padding: 0.1rem 0.35rem;
    border-radius: 4px;
    border: 1px solid rgba(255, 255, 255, 0.25);
    background: #1e1e1e;
    font-size: 0.8rem;
    font-family: inherit;
    color: #ffffff;
  }

  .controls {
    display: flex;
    gap: 0.75rem;
  }

  .controls button {
    padding: 0.8rem 1.1rem;
    min-width: 110px;
  }

  .controls button:disabled {
    opacity: 0.4;
    cursor: default;
    transform: none;
  }

  .page-stage {
    display: grid;
    place-items: center;
    min-width: 0;
    min-height: 0;
    padding: 1.25rem;
    overflow: hidden;
  }

  .spread {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 1.25rem;
    width: 100%;
    height: 100%;
    align-items: center;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }

  .spread.single-page {
    grid-template-columns: minmax(0, 1fr);
    max-width: min(100%, 900px);
  }

  .reader.bed-reader-mode .spread.single-page {
    max-width: none;
  }

  .page-stage.bed-mode {
    container-type: size;
    padding: 0.35rem;
  }

  .reader.bed-reader-mode .bed-spin {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 100%;
    height: 100%;
    min-width: 0;
    min-height: 0;
  }

  .reader.bed-reader-mode .page-image-bed--cw,
  .reader.bed-reader-mode .page-image-bed--ccw {
    width: auto;
    height: auto;
    max-width: 96dvh;
    max-height: 96dvw;
    object-fit: contain;
  }

  .reader.bed-reader-mode .page-image-bed--cw {
    transform: rotate(90deg);
  }

  .reader.bed-reader-mode .page-image-bed--ccw {
    transform: rotate(-90deg);
  }

  @supports (height: 1cqh) {
    .reader.bed-reader-mode .page-image-bed--cw,
    .reader.bed-reader-mode .page-image-bed--ccw {
      max-width: min(100cqh, 96dvh);
      max-height: min(100cqw, 96dvw);
    }
  }

  .page-image {
    display: block;
    width: 100%;
    height: 100%;
    min-width: 0;
    min-height: 0;
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
    box-shadow: 0 24px 60px rgba(0, 0, 0, 0.42);
    border-radius: 4px;
    user-select: none;
    -webkit-user-drag: none;
  }

  .status.error {
    padding: 0 1.25rem;
    color: #ffb3b3;
  }

  @media (max-width: 900px) {
    .app-shell {
      grid-template-columns: 220px 1fr;
    }

    .spread {
      gap: 0.75rem;
    }
  }
</style>
