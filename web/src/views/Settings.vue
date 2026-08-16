<template>
  <div class="page">
    <div class="page-head">
      <div>
        <div class="page-title">设置</div>
        <div class="page-sub">服务端与匹配引擎配置</div>
      </div>
      <button class="btn btn-primary" @click="save">💾 保存设置</button>
    </div>

    <div class="settings-grid">
      <!-- 服务设置 -->
      <div class="card set-card">
        <div class="sc-title">⚙️ 服务设置</div>
        <div class="sc-body">
          <div class="form-item">
            <label>监听端口</label>
            <input v-model="cfg.port" class="input mono" />
          </div>
          <div class="form-item">
            <label>管理员密码</label>
            <input v-model="cfg.pass" class="input" type="password" />
          </div>
          <div class="form-item">
            <label>数据目录</label>
            <input v-model="cfg.dataDir" class="input mono" />
          </div>
        </div>
      </div>

      <!-- 匹配引擎 -->
      <div class="card set-card">
        <div class="sc-title">🔍 匹配引擎</div>
        <div class="sc-body">
          <div class="form-item">
            <label>默认匹配阈值 <span class="mono accent">{{ cfg.threshold }}</span></label>
            <input v-model.number="cfg.threshold" type="range" class="range" min="0.5" max="0.99" step="0.01" />
          </div>
          <div class="form-item">
            <label>截图缓存 <span class="desc">从视频流软解码取帧，找图延迟 &lt;50ms（需 ffmpeg）</span></label>
            <label class="switch">
              <input type="checkbox" v-model="cfg.frameCache" />
              <span class="track"></span>
            </label>
          </div>
          <div class="form-item">
            <label>找图超时上限（ms）</label>
            <input v-model="cfg.timeout" class="input mono" />
          </div>
          <div class="form-item">
            <label>匹配失败重试次数</label>
            <input v-model="cfg.retries" class="input mono" />
          </div>
        </div>
      </div>

      <!-- 连接设置 -->
      <div class="card set-card">
        <div class="sc-title">🎥 画面设置</div>
        <div class="sc-body">
          <div class="form-item">
            <label>分辨率上限</label>
            <select v-model="cfg.maxRes" class="select">
              <option value="0">原始分辨率</option>
              <option value="1920">1920</option>
              <option value="1080">1080</option>
              <option value="720">720</option>
            </select>
          </div>
          <div class="form-item">
            <label>码率上限（Mbps）</label>
            <input v-model="cfg.bitrate" class="input mono" />
          </div>
          <div class="form-item">
            <label>帧率上限</label>
            <input v-model="cfg.fps" class="input mono" />
          </div>
          <div class="form-item">
            <label>视频流软解码（供匹配取帧）</label>
            <label class="switch">
              <input type="checkbox" v-model="cfg.decode" />
              <span class="track"></span>
            </label>
          </div>
        </div>
      </div>

      <!-- 关于 -->
      <div class="card set-card">
        <div class="sc-title">ℹ️ 关于</div>
        <div class="sc-body about">
          <div class="about-logo">🎮</div>
          <div class="about-name">GameBot 游戏自动化助手</div>
          <div class="about-ver mono">v0.1.0</div>
          <div class="about-desc">
            基于 scrcpy（官方开源 server）+ Rust 服务端 + WebRTC 的轻量游戏自动化方案。
            <br />YAML 脚本 · 模板匹配 · 定时任务 · Docker 部署。
          </div>
          <div class="about-stack mono">
            Rust (axum + webrtc-rs) · Vue 3 · scrcpy-server · adb
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup>
import { reactive } from 'vue'
import { useToast } from '../store'

const toast = useToast()
const cfg = reactive({
  port: 8443,
  pass: 'admin123',
  dataDir: '/app/data',
  threshold: 0.85,
  frameCache: true,
  timeout: 10000,
  retries: 2,
  maxRes: '0',
  bitrate: 20,
  fps: 60,
  decode: true
})

function save() { toast('设置已保存（原型）', 'success') }
</script>

<style scoped>
.settings-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); gap: 14px; }
.set-card { display: flex; flex-direction: column; gap: 14px; }
.sc-title { font-size: 14px; font-weight: 700; }
.sc-body { display: flex; flex-direction: column; gap: 14px; }
.desc { color: var(--text-2); font-size: 11px; }
.accent { color: var(--accent); }

.about { align-items: center; text-align: center; gap: 6px; }
.about-logo { font-size: 40px; }
.about-name { font-size: 16px; font-weight: 700; }
.about-ver { color: var(--text-2); font-size: 12px; }
.about-desc { color: var(--text-1); font-size: 12px; line-height: 1.7; }
.about-stack { color: var(--text-2); font-size: 11px; margin-top: 6px; }
</style>
