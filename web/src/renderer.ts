// Canvas framebuffer 渲染器(设计文档 §11)。
// Canvas 即最终 framebuffer:RECT 解码绘制、COPY 本地复制。

import { Codec, FrameMsg, Op } from "./protocol";
import { decompress } from "fzstd";

export class Renderer {
  readonly canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private scratch: HTMLCanvasElement;
  private scratchCtx: CanvasRenderingContext2D;
  private fbW = 0;
  private fbH = 0;
  /** 每帧 op 应用完成后的回调(SDK 多视图同步重绘用;主应用不设置) */
  onPaint: (() => void) | null = null;
  /** 顺序化:异步解码(webp bitmap)不乱序 */
  private chain: Promise<void> = Promise.resolve();

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d", { alpha: false, desynchronized: true })!;
    this.scratch = document.createElement("canvas");
    this.scratchCtx = this.scratch.getContext("2d", { alpha: false })!;
  }

  get framebufferSize(): { w: number; h: number } {
    return { w: this.fbW, h: this.fbH };
  }

  init(w: number, h: number): void {
    this.fbW = w;
    this.fbH = h;
    this.canvas.width = w;
    this.canvas.height = h;
    this.ctx.fillStyle = "#000";
    this.ctx.fillRect(0, 0, w, h);
  }

  applyFrame(frame: FrameMsg): void {
    this.chain = this.chain
      .then(() => this.applyOps(frame.ops))
      .then(() => this.onPaint?.())
      .catch((e) => console.error("applyFrame error", e));
  }

  private async applyOps(ops: Op[]): Promise<void> {
    for (const op of ops) {
      if (op.kind === "rect") {
        await this.applyRect(op);
      } else if (op.kind === "copy") {
        this.applyCopy(op);
      }
      // kind === "cursor":协议保留,忽略 —— 指针由浏览器本地渲染
      // (服务端已 XFixesHideCursor,画面本身不含光标)
    }
  }

  private async applyRect(op: Extract<Op, { kind: "rect" }>): Promise<void> {
    const { x, y, w, h, pw, ph, codec, payload } = op;
    if (pw === 0 || ph === 0) return;
    if (codec === Codec.Webp) {
      // 浏览器原生 WebP 解码(V2 §12:不引入 WASM 解码器)
      const bitmap = await createImageBitmap(
        new Blob([payload as BlobPart], { type: "image/webp" }),
      );
      this.ctx.imageSmoothingEnabled = true;
      this.ctx.drawImage(bitmap, x, y, w, h);
      bitmap.close();
      return;
    }

    let rgba = payload;
    if (codec === Codec.Zstd) {
      rgba = decompress(payload);
    }
    if (rgba.length < pw * ph * 4) return;

    if (pw === w && ph === h) {
      // 未缩放:putImageData 快路径
      this.ctx.putImageData(new ImageData(
          new Uint8ClampedArray(rgba.buffer as ArrayBuffer, rgba.byteOffset, pw * ph * 4),
          pw,
          ph,
        ), x, y);
    } else {
      // 画面质量 < 100%:先放 scratch,再放大到原坐标
      if (this.scratch.width !== pw || this.scratch.height !== ph) {
        this.scratch.width = pw;
        this.scratch.height = ph;
      }
      this.scratchCtx.putImageData(
        new ImageData(
          new Uint8ClampedArray(rgba.buffer as ArrayBuffer, rgba.byteOffset, pw * ph * 4),
          pw,
          ph,
        ),
        0,
        0,
      );
      this.ctx.imageSmoothingEnabled = true;
      this.ctx.drawImage(this.scratch, 0, 0, pw, ph, x, y, w, h);
    }
  }

  private applyCopy(op: Extract<Op, { kind: "copy" }>): void {
    this.ctx.drawImage(
      this.canvas,
      op.srcX,
      op.srcY,
      op.w,
      op.h,
      op.dstX,
      op.dstY,
      op.w,
      op.h,
    );
  }
}
