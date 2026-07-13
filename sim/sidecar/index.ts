#!/usr/bin/env bun
/**
 * MetroForge sim sidecar — Bun.serve WebSocket entry point (mf-wire v1).
 * v1 scope: a single sim per process, single connected client. On listen it
 * prints exactly one stdout line so a parent process (or the smoke test) can
 * discover the assigned port without scraping logs.
 */
import { WORLD_SIZE } from '@core/constants';
import pkgJson from '../package.json';
import { CITY_LIST } from './cities';
import { SimHost } from './simHost';
import { BACKPRESSURE_LIMIT_BYTES, decodeEnvelope, jsonMessage, PROTOCOL_VERSION, type OutMessage } from './wire';

interface CliArgs {
  port: number;
  headlessSpeed: number | undefined;
}

function parseArgs(argv: string[]): CliArgs {
  let port = 0; // 0 = OS-assigned
  let headlessSpeed: number | undefined;
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--port' && argv[i + 1] !== undefined) {
      port = Number(argv[++i]);
    } else if (arg === '--headless-speed' && argv[i + 1] !== undefined) {
      headlessSpeed = Number(argv[++i]);
    }
  }
  return { port, headlessSpeed };
}

interface WsData {
  host: SimHost | null;
}

const { port, headlessSpeed } = parseArgs(process.argv.slice(2));
const gameVersion = (pkgJson as { version: string }).version;

const server = Bun.serve<WsData>({
  port,
  fetch(req, srv) {
    if (srv.upgrade(req, { data: { host: null } })) return undefined;
    return new Response('Upgrade required', { status: 400 });
  },
  websocket: {
    open(ws) {
      const send = (msg: OutMessage): void => {
        if (msg.droppable && ws.getBufferedAmount() > BACKPRESSURE_LIMIT_BYTES) return;
        if (msg.kind === 'text') ws.send(msg.json);
        else ws.send(msg.buf);
      };
      const host = new SimHost(send, () => {
        ws.close(1000, 'shutdown');
        process.exit(0);
      });
      ws.data.host = host;
      host.start();
      if (headlessSpeed !== undefined) {
        host.setSpeed(headlessSpeed);
        host.setStepCap(Infinity);
      }
      send(
        jsonMessage('hello', {
          protocolVersion: PROTOCOL_VERSION,
          gameVersion,
          cityList: CITY_LIST,
          defaultWorldSize: WORLD_SIZE,
        }),
      );
    },
    message(ws, message) {
      if (typeof message !== 'string') return; // binary inbound unused in v1
      const host = ws.data.host;
      if (!host) return;
      host.handleEnvelope(decodeEnvelope(message));
    },
    close(ws) {
      ws.data.host?.stop();
      ws.data.host = null;
    },
  },
});

// Exactly one stdout line — the handshake a spawning parent (native client or
// smoke test) reads to learn the OS-assigned port.
console.log(JSON.stringify({ mf: 'sidecar', protocolVersion: PROTOCOL_VERSION, port: server.port, pid: process.pid }));
