// WebDesk 二进制协议(与服务端 src/protocol/ 严格对应)。
// 所有整数小端;像素 RGBA8888。

export const PROTOCOL_VERSION = 2;

// 服务端 -> 客户端
export const MSG_HELLO = 0x01;
export const MSG_FRAME = 0x02;
export const MSG_CLIPBOARD = 0x15;

// 客户端 -> 服务端
export const MSG_MOUSE = 0x10;
export const MSG_KEY = 0x11;
export const MSG_SETTINGS = 0x12;
export const MSG_CLIPBOARD_SET = 0x13;
export const MSG_CLIPBOARD_GET = 0x14;
export const MSG_KEEPALIVE = 0x16;

export const enum Codec {
  Raw = 0,
  Zstd = 1,
  /** V2:有损压缩为 WebP(codec id 2,顶替 V1 的 JPEG) */
  Webp = 2,
}

export const enum MouseAction {
  Move = 0,
  Down = 1,
  Up = 2,
  Wheel = 3,
}

export interface HelloMsg {
  width: number;
  height: number;
}

export type Op =
  | {
      kind: "rect";
      x: number;
      y: number;
      w: number;
      h: number;
      pw: number;
      ph: number;
      codec: Codec;
      payload: Uint8Array;
    }
  | {
      kind: "copy";
      srcX: number;
      srcY: number;
      dstX: number;
      dstY: number;
      w: number;
      h: number;
    }
  | { kind: "cursor"; x: number; y: number };

export interface FrameMsg {
  seq: number;
  ts: number;
  ops: Op[];
}

export type ServerMsg =
  | { type: "hello"; hello: HelloMsg }
  | { type: "frame"; frame: FrameMsg }
  | { type: "clipboard"; text: string };

/** 解码一条服务端消息。 */
export function decodeServer(data: Uint8Array): ServerMsg {
  const dv = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const type = dv.getUint8(0);
  switch (type) {
    case MSG_HELLO: {
      return {
        type: "hello",
        hello: {
          width: dv.getUint16(3, true),
          height: dv.getUint16(5, true),
        },
      };
    }
    case MSG_FRAME: {
      const seq = dv.getUint32(1, true);
      const ts = dv.getBigUint64(5, true);
      const opCount = dv.getUint16(13, true);
      let p = 15;
      const ops: Op[] = [];
      for (let i = 0; i < opCount; i++) {
        const opType = dv.getUint8(p);
        p += 1;
        // 注:opType 已消费,p 相对各自字段起点
        if (opType === 1) {
          // RECT: [x y w h pw ph][codec][len][payload](相对 op 起点 +1)
          const x = dv.getInt16(p, true);
          const y = dv.getInt16(p + 2, true);
          const w = dv.getUint16(p + 4, true);
          const h = dv.getUint16(p + 6, true);
          const pw = dv.getUint16(p + 8, true);
          const ph = dv.getUint16(p + 10, true);
          const codec = dv.getUint8(p + 12) as Codec;
          const len = dv.getUint32(p + 13, true);
          const payload = data.slice(p + 17, p + 17 + len);
          p += 17 + len;
          ops.push({ kind: "rect", x, y, w, h, pw, ph, codec, payload });
        } else if (opType === 2) {
          // COPY: 12 字节
          const srcX = dv.getInt16(p, true);
          const srcY = dv.getInt16(p + 2, true);
          const dstX = dv.getInt16(p + 4, true);
          const dstY = dv.getInt16(p + 6, true);
          const w = dv.getUint16(p + 8, true);
          const h = dv.getUint16(p + 10, true);
          p += 12;
          ops.push({ kind: "copy", srcX, srcY, dstX, dstY, w, h });
        } else if (opType === 3) {
          // CURSOR: 4 字节
          const x = dv.getInt16(p, true);
          const y = dv.getInt16(p + 2, true);
          p += 4;
          ops.push({ kind: "cursor", x, y });
        } else {
          throw new Error(`unknown op type ${opType}`);
        }
      }
      return { type: "frame", frame: { seq, ts: Number(ts), ops } };
    }
    case MSG_CLIPBOARD: {
      const len = dv.getUint32(1, true);
      const text = new TextDecoder().decode(data.subarray(5, 5 + len));
      return { type: "clipboard", text };
    }
    default:
      throw new Error(`unknown message type ${type}`);
  }
}

function pushU16(out: number[], v: number) {
  out.push(v & 0xff, (v >>> 8) & 0xff);
}
function pushU32(out: number[], v: number) {
  out.push(v & 0xff, (v >>> 8) & 0xff, (v >>> 16) & 0xff, (v >>> 24) & 0xff);
}

export function encodeMouse(
  action: MouseAction,
  button: number,
  x: number,
  y: number,
  wheelDx = 0,
  wheelDy = 0,
): Uint8Array {
  const out: number[] = [MSG_MOUSE, action, button];
  pushU16(out, x);
  pushU16(out, y);
  pushU16(out, wheelDx);
  pushU16(out, wheelDy);
  return new Uint8Array(out);
}

export function encodeKey(pressed: boolean, keysym: number): Uint8Array {
  const out: number[] = [MSG_KEY, pressed ? 0 : 1, 0, 0];
  pushU32(out, keysym);
  return new Uint8Array(out);
}

/**
 * 画质 + 当前 FPS 上限(0.8.0 起追加第 6 字节 fps)。
 * FPS 完全由前端控制:交互时报 maxFps,空闲倒计时到点报 idleFps;
 * 服务端纯执行,没有高/低档概念。旧 agent(5 字节)忽略 fps 字节照常工作。
 */
export function encodeSettings(
  scale: number,
  compression: number,
  idleFps: number,
  maxFps: number,
  fps: number,
): Uint8Array {
  return new Uint8Array([MSG_SETTINGS, scale, compression, idleFps, maxFps, fps]);
}

export function encodeClipboardSet(text: string): Uint8Array {
  const bytes = new TextEncoder().encode(text);
  const out: number[] = [MSG_CLIPBOARD_SET];
  pushU32(out, bytes.length);
  return new Uint8Array([...out, ...bytes]);
}

export const CLIPBOARD_GET = new Uint8Array([MSG_CLIPBOARD_GET]);

/** 链路保活(客户端每 25s 一帧):让反代/NAT 持续看到客户端方向的数据 */
export const KEEPALIVE = new Uint8Array([MSG_KEEPALIVE]);
