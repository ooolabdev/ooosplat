// Pointer-lock ownership is local to a preview. Keep a cancelled, pending legacy
// (void-returning) request guarded until the browser reports its outcome, even
// after the preview is disposed. It must never release another canvas's lock.
const pendingRequests = new WeakMap<Document, ViewerPointerLock>();

export class ViewerPointerLock {
  private readonly document: Document;
  private pending = false;
  private wanted = false;
  private active = false;
  private disposed = false;
  private requestId = 0;
  private promisePending = false;

  constructor(private readonly canvas: HTMLCanvasElement, private onLost: (() => void) | null) {
    this.document = canvas.ownerDocument;
    this.document.addEventListener("pointerlockchange", this.onChange);
    this.document.addEventListener("pointerlockerror", this.onError);
  }

  ownsLock() {
    const owned = this.document.pointerLockElement === this.canvas;
    // Consumers can observe the actual owner before the queued change event.
    // Remember that grant so a subsequent loss still stops keyboard movement.
    if (owned && this.wanted && !this.disposed) {
      this.active = true;
      this.settled();
    }
    return owned;
  }

  request() {
    if (this.disposed || this.pending || pendingRequests.has(this.document) || this.document.pointerLockElement) return;
    if (typeof this.canvas.requestPointerLock !== "function") {
      console.debug("[preview] Pointer Lock is unavailable; retaining keyboard/drag navigation.");
      return;
    }
    this.wanted = true;
    this.pending = true;
    this.promisePending = false;
    const requestId = ++this.requestId;
    pendingRequests.set(this.document, this);
    try {
      // Must stay in the trusted keydown handler, before any await or RAF.
      const result = this.canvas.requestPointerLock();
      if (result && typeof result.then === "function") {
        this.promisePending = true;
        void result.then(() => {
          if (requestId !== this.requestId) return;
          this.promisePending = false;
          this.settled(); this.onChange();
        }, error => { if (requestId === this.requestId) this.failed(error); });
      }
    } catch (error) {
      this.failed(error);
    }
  }

  release() {
    this.wanted = false;
    this.active = false;
    if (this.ownsLock()) {
      this.settled();
      try { this.document.exitPointerLock(); }
      catch (error) { console.debug("[preview] Unable to release Pointer Lock.", error); }
    }
  }

  destroy() {
    this.disposed = true;
    this.onLost = null;
    this.release();
    this.cleanup();
  }

  private settled() {
    this.pending = false;
    if (pendingRequests.get(this.document) === this) pendingRequests.delete(this.document);
  }

  private cleanup() {
    if (!this.disposed || this.pending) return;
    this.document.removeEventListener("pointerlockchange", this.onChange);
    this.document.removeEventListener("pointerlockerror", this.onError);
  }

  private onChange = () => {
    if (this.ownsLock()) {
      this.settled();
      if (!this.wanted || this.disposed) this.release();
      else this.active = true;
    } else if (this.active) {
      this.active = false;
      this.wanted = false;
      this.onLost?.();
    }
    this.cleanup();
  };

  // Promise implementations carry the request identity; an unlabelled document
  // error event must not reject a later request on the same preview.
  private onError = () => { if (this.pending && !this.promisePending) this.failed(); };

  private failed = (error?: unknown) => {
    this.settled();
    this.promisePending = false;
    this.wanted = false;
    console.debug("[preview] Pointer Lock was denied; retaining keyboard/drag navigation.", error ?? "");
    this.cleanup();
  };
}
