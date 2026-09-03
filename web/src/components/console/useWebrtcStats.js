/**
 * WebRTC 传输统计与画面自愈看门狗（1s 轮询）：
 * 黑屏两级处理、静默检测、拖动后画面停滞探测、jitter buffer 延迟看门狗、
 * 码率/帧率/延迟展示、PLI 花屏自愈。自 Console.vue 原样拆出，行为零变化。
 */
export function useWebrtcStats({
  getPeerConnection,
  connected,
  videoElement,
  fps,
  delay,
  bitrate,
  sendControl,
  handleVideoSilence,
  // 连接时间戳与最近拖动输入时间仍由 Console 持有（连接生命周期的一部分），经 getter 注入。
  getVideoConnectTs,
  getLastDragInputAt,
}) {
  let statsTimer = null
  let delaySpikes = 0
  let hadVideo = false
  let videoBytesAdvanced = false
  let lastVideoTime = 0
  let stillFrames = 0
  let lastBytesReceived = 0
  let lastBitrateTs = 0
  let lastJbd = 0
  let lastJbe = 0
  let lastPliCount = 0
  let lastPliResetAt = 0
  let pliResetStreak = 0
  let renderFpLast = ''
  let renderFpFrozen = 0
  let fpCanvas = null
  let fpCtx = null

  /** 格式化传输码率 */
  function formatBitrate(bps) {
    if (!bps || bps <= 0) return '—'
    if (bps >= 1000000) return (bps / 1000000).toFixed(1) + ' Mbps'
    if (bps >= 1000) return Math.round(bps / 1000) + ' Kbps'
    return Math.round(bps) + ' bps'
  }

  function startStats() {
    if (statsTimer) clearInterval(statsTimer)
    statsTimer = setInterval(async () => {
      if (!getPeerConnection()) return
      const v = videoElement.value
      // 两级黑屏处理（静态屏 MTK 编码器对 reset 响应极慢，实测要多次才吐 IDR）：
      // 8s 仍无可解码帧 → 先补一次 reset_video 要 IDR，继续等到 16s；仍黑屏才重连。
      // 旧实现 8s 直接重连：编码器还没来得及响应就被拆链，静态屏下形成重连风暴
      if (connected.value && v && !hadVideo && v.videoWidth === 0 && Date.now() - getVideoConnectTs() > 8000) {
        const blackFor = Date.now() - getVideoConnectTs()
        if (!blackResetSent && blackFor < 16000) {
          blackResetSent = true
          console.warn('[webrtc] no decodable video after 8s, requesting IDR via reset_video')
          sendControl({ type: 'reset_video' })
          return
        }
        if (blackFor >= 16000) {
          console.warn('[webrtc] no decodable video after 16s, reconnecting')
          handleVideoSilence()
          return
        }
      }
      // 视频静默检测：仅在见过画面后启用（连接初期 currentTime=0 不误判）
      if (connected.value && v && v.videoWidth > 0) {
        hadVideo = true
        if (Math.abs(v.currentTime - lastVideoTime) < 0.001 && !videoBytesAdvanced) {
          if (++stillFrames >= 2) { // 连续 ~4s：currentTime 冻结且零新增字节
            stillFrames = 0
            handleVideoSilence()
          }
        } else {
          stillFrames = 0
          lastVideoTime = v.currentTime
        }
        // 画面停滞看门狗（原理见变量处注释）：渲染像素指纹连续未变 + 近期有
        // 拖动/滚轮输入（画面本应变化）。先 reset_video，5s 后仍冻结才重连，
        // 避免对"拖不动/本就静止"的界面频繁误重连
        if (connected.value && hadVideo) {
          let fp = ''
          try {
            if (!fpCanvas) {
              fpCanvas = document.createElement('canvas')
              fpCanvas.width = 24; fpCanvas.height = 14
              fpCtx = fpCanvas.getContext('2d', { willReadFrequently: true })
            }
            fpCtx.drawImage(v, 0, 0, 24, 14)
            const d = fpCtx.getImageData(0, 0, 24, 14).data
            let h = 5381
            for (let i = 0; i < d.length; i += 4) h = ((h * 33) ^ (d[i] + d[i+1] + d[i+2])) >>> 0
            fp = String(h)
          } catch (err) { /* drawImage 失败（如视频未就绪）跳过本轮 */ }
          if (fp && fp === renderFpLast) {
            renderFpFrozen++
          } else {
            renderFpFrozen = 0
            if (fp) stallResetSent = false
          }
          renderFpLast = fp
          if (renderFpFrozen >= 5 && Date.now() - getLastDragInputAt() < 8000) {
            renderFpFrozen = 0
            if (!stallResetSent) {
              stallResetSent = true
              console.warn('[webrtc] picture frozen after drag/scroll input, requesting IDR via reset_video')
              sendControl({ type: 'reset_video' })
            } else {
              stallResetSent = false
              console.warn('[webrtc] picture still frozen after reset_video, reconnecting to rebuild jitter buffer')
              handleVideoSilence()
            }
          }
        }
      }
      try {
        const stats = await getPeerConnection().getStats()
        let fpsCount = 0
        stats.forEach(s => {
          if (s.type === 'inbound-rtp' && s.kind === 'video') {
            if (s.framesPerSecond) fpsCount = Math.round(s.framesPerSecond)
            // 画面延迟：jitterBufferDelay 规范单位为秒（个别 Chromium 版本报 ms，自适应：
            // 单帧均值 >50s 视为 ms 直读，否则按秒换算）。只统计增量窗口，避免累计均值失真
            if (typeof s.jitterBufferDelay === 'number' && s.jitterBufferEmittedCount > 0) {
              if (lastJbe > 0 && s.jitterBufferEmittedCount > lastJbe) {
                const perFrame = (s.jitterBufferDelay - lastJbd) / (s.jitterBufferEmittedCount - lastJbe)
                if (perFrame >= 0 && perFrame < 50) {
                  delay.value = Math.round(perFrame * 1000)
                }
                // 延迟看门狗：音频轨 A/V 同步异常会把 jitter buffer 目标延迟单调拉高
                // （挂机静止画面 87ms → 3s+ 且不回落，见 AGENTS.md 已知坑）。连续两次
                // 采样超阈值（~4s）→ 走断流重连路径重置缓冲（含页面锁二次检查）
                if (delay.value > 1500) {
                  if (++delaySpikes >= 2) {
                    delaySpikes = 0
                    console.warn('[webrtc] latency watchdog: delay=' + delay.value + 'ms, reconnecting')
                    handleVideoSilence()
                    return
                  }
                } else {
                  delaySpikes = 0
                }
              }
              lastJbd = s.jitterBufferDelay
              lastJbe = s.jitterBufferEmittedCount
            }
            // 传输码率：按字节增量 / 时间增量估算
            if (typeof s.bytesReceived === 'number') {
              const now = Date.now()
              if (lastBytesReceived > 0 && lastBitrateTs > 0) {
                const dt = (now - lastBitrateTs) / 1000
                if (dt > 0) bitrate.value = formatBitrate(((s.bytesReceived - lastBytesReceived) * 8) / dt)
              }
              // 链路活性（静默检测双条件用，见检测处注释）；重连后计数回退
              videoBytesAdvanced = s.bytesReceived > lastBytesReceived
              if (s.bytesReceived < lastBytesReceived) lastBytesReceived = 0
              lastBytesReceived = s.bytesReceived
              lastBitrateTs = now
            }
            // 花屏自愈：解码器失步（PLI 增量）→ 请求设备立即出关键帧。
            // 限频 2s：持续丢包（WiFi 差）时最多每 2s 重置一次，不会打爆编码器
            if (typeof s.pliCount === 'number') {
              if (s.pliCount < lastPliCount) lastPliCount = s.pliCount // 重连后回退
              if (s.pliCount > lastPliCount) {
                lastPliCount = s.pliCount
                // 连接初期（~6s 内）Chrome 加入流时会例行发 PLI 请求关键帧，不是失步：
                // 静态屏（无应用/挂机静止）编码器对 reset 响应极慢（MTK 要多次才吐
                // IDR），reset 反而打断静止补帧 → 浏览器断供 4s 被静默检测杀掉 →
                // "连上一会儿就断"死循环。真失步（解码中突发花屏）不受此窗口限制
                const joinWindow = Date.now() - getVideoConnectTs() < 6000
                const now = Date.now()
                const backoff = pliResetStreak >= 4 ? 60000 : pliResetStreak >= 2 ? 15000 : 2000
                if (!joinWindow && connected.value && now - lastPliResetAt > backoff) {
                  lastPliResetAt = now
                  pliResetStreak++
                  console.warn('[webrtc] decoder desync (pliCount=' + s.pliCount + ', streak=' + pliResetStreak + '), requesting IDR via reset_video')
                  sendControl({ type: 'reset_video' })
                }
              } else if (s.pliCount === lastPliCount && lastPliCount > 0) {
                // 一整个统计周期无新 PLI：解码器已满足，退避复位
                pliResetStreak = 0
              }
            }
            // 诊断：每 3 次打印一次接收统计
            if (!window.__rtpStatsCount) window.__rtpStatsCount = 0
            if (++window.__rtpStatsCount % 3 === 0) {
              const v = videoElement.value
              console.log('[webrtc] inbound-rtp', JSON.stringify({
                bytesReceived: s.bytesReceived, packetsReceived: s.packetsReceived,
                framesDecoded: s.framesDecoded, framesDropped: s.framesDropped,
                framesPerSecond: s.framesPerSecond, keyFramesDecoded: s.keyFramesDecoded,
                pliCount: s.pliCount, nackCount: s.nackCount,
                codecId: s.codecId, decoder: s.decoderImplementation,
                videoWidth: v?.videoWidth, videoHeight: v?.videoHeight, readyState: v?.readyState
              }))
            }
          }
        })
        if (fpsCount) fps.value = fpsCount
      } catch (e) {}
      // 1s 轮询：比 2s 更快发现花屏（PLI 自愈延迟减半）与延迟/静默异常
    }, 1000)
  }

  function stopStats() {
    if (statsTimer) {
      clearInterval(statsTimer)
      statsTimer = null
    }
  }

  // 连接时间戳与最近拖动输入时间仍由 Console 持有（连接生命周期的一部分），经注入读取。
  let blackResetSent = false
  let stallResetSent = false

  /** DataChannel 重建（重连成功）时允许黑屏看门狗再走一轮两级 reset 处理。 */
  function resetBlackWatchdog() {
    blackResetSent = false
  }

  /** 断连后复位看门狗状态（与原 Console onDisconnect 清理项一致）。 */
  function resetWatchdogs() {
    hadVideo = false
    stillFrames = 0
    renderFpLast = ''
    renderFpFrozen = 0
    stallResetSent = false
    blackResetSent = false
    lastBytesReceived = 0
    lastBitrateTs = 0
    bitrate.value = '—'
  }

  return { startStats, stopStats, resetWatchdogs, resetBlackWatchdog, formatBitrate }
}
