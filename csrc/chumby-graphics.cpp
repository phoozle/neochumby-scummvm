/* neochumby-scummvm: ScummVM's graphics on the Chumby's 320x240 RGB565
 * screen (see chumby-graphics.h).
 *
 * Copyright (C) 2026 the neochumby-scummvm authors
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version. See LICENSE.
 *
 * Everything per pixel here is integer: the ARM926 has no FPU.
 */

#define FORBIDDEN_SYMBOL_ALLOW_ALL
#include "common/scummsys.h"

#include "chumby-graphics.h"
#include "chumby.h"

#include "common/system.h"
#include "common/textconsole.h"

#include <string.h>

/* The 2:1 downscale averages four RGB565 pixels at once by "spreading"
 * each into 32 bits, green moved up to bits 21-26 so that every component
 * has empty bits above it:
 *
 *   bits 0-4 blue, 11-15 red, 21-26 green
 *
 * Four spread pixels add up without one component carrying into the next
 * (blue and red reach 7 bits, green 8), so a 2x2 block is four table
 * lookups, three additions, a rounding bias, a shift and a mask. */
static inline uint32 spread(uint16 c) {
	return (c & 0xF81F) | ((uint32)(c & 0x07E0) << 16);
}

/* The average of four spread pixels, rounded, back in RGB565. The bias is
 * 2 in each component's lowest bit: 2 << 0, 2 << 11 and 2 << 21. */
static inline uint16 fold4(uint32 sum) {
	uint32 t = ((sum + 0x00401002) >> 2) & 0x07E0F81F;
	return (uint16)(t | (t >> 16));
}

static inline uint16 rgb565(byte r, byte g, byte b) {
	return (uint16)(((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3));
}

/* RGB555 (the HE99+ games' 16-bit format) to RGB565: green's top bit is
 * copied into its new lowest bit, so white stays white. */
static inline uint16 from555(uint16 c) {
	return (uint16)(((c & 0x7FE0) << 1) | (c & 0x1F) | ((c >> 4) & 0x20));
}

static const Graphics::PixelFormat kFormat565(2, 5, 6, 5, 0, 11, 5, 0, 0);
static const Graphics::PixelFormat kFormat555(2, 5, 5, 5, 0, 10, 5, 0, 0);

/* How many of the four pixels of a 2x2 block of a halved cursor must be
 * opaque for the block to be: two keeps the outline of a cursor drawn
 * with lines one pixel wide, without thickening it. */
static const int kCursorOpaqueOfFour = 2;

/* More separate changed rectangles than this in one frame and the whole
 * screen is redrawn instead. */
static const uint kMaxDirty = 32;

ChumbyGraphicsManager::ChumbyGraphicsManager()
	: _format(Graphics::PixelFormat::createFormatCLUT8()), _overlayVisible(false), _screenChangeID(0),
	  _transactionError(OSystem::kTransactionSuccess), _scale(1), _xoff(0), _yoff(0), _shakeY(0), _allDirty(true),
	  _cursorW(0), _cursorH(0), _cursorHotX(0), _cursorHotY(0), _cursorKey(0),
	  _cursorFormat(Graphics::PixelFormat::createFormatCLUT8()), _cursorPaletteEnabled(false), _cursorStale(true),
	  _cursorScale(1), _cursorDrawW(0), _cursorDrawH(0), _cursorDrawHotX(0), _cursorDrawHotY(0), _cursorVisible(false),
	  _mouseX(0), _mouseY(0), _cursorMoved(false) {
	memset(_palette, 0, sizeof(_palette));
	memset(_pal565, 0, sizeof(_pal565));
	memset(_palSpread, 0, sizeof(_palSpread));
	memset(_cursorPalette, 0, sizeof(_cursorPalette));
	_overlay.create(NC_W, NC_H, kFormat565);
}

ChumbyGraphicsManager::~ChumbyGraphicsManager() {
	_game.free();
	_overlay.free();
}

bool ChumbyGraphicsManager::hasFeature(OSystem::Feature f) const {
	return f == OSystem::kFeatureCursorPalette || f == OSystem::kFeatureCursorMask;
}

void ChumbyGraphicsManager::setFeatureState(OSystem::Feature f, bool enable) {
	if (f == OSystem::kFeatureCursorPalette && enable != _cursorPaletteEnabled) {
		_cursorPaletteEnabled = enable;
		_cursorStale = true;
	}
}

bool ChumbyGraphicsManager::getFeatureState(OSystem::Feature f) const {
	return f == OSystem::kFeatureCursorPalette && _cursorPaletteEnabled;
}

#ifdef USE_RGB_COLOR
Common::List<Graphics::PixelFormat> ChumbyGraphicsManager::getSupportedFormats() const {
	Common::List<Graphics::PixelFormat> list;
	list.push_back(kFormat565);
	list.push_back(kFormat555);
	list.push_back(Graphics::PixelFormat::createFormatCLUT8());
	return list;
}
#endif

void ChumbyGraphicsManager::initSize(uint width, uint height, const Graphics::PixelFormat *format) {
	Graphics::PixelFormat f = format ? *format : Graphics::PixelFormat::createFormatCLUT8();
	_transactionError = OSystem::kTransactionSuccess;
	if (!f.isCLUT8() && f != kFormat565 && f != kFormat555) {
		warning("chumby: pixel format %s is not supported", f.toString().c_str());
		_transactionError = OSystem::kTransactionFormatNotSupported;
		f = Graphics::PixelFormat::createFormatCLUT8();
	}
	if (_game.getPixels() && (uint)_game.w == width && (uint)_game.h == height && _format == f)
		return;
	_format = f;
	_game.free();
	_game.create(width, height, f);
	// Halved when it is bigger than the screen (the HE games' 640x480), else
	// as it is (the 320x200 SCUMM games), centred either way.
	_scale = (width > NC_W || height > NC_H) ? 2 : 1;
	_xoff = MAX<int>(0, (NC_W - (int)width / _scale) / 2);
	_yoff = MAX<int>(0, (NC_H - (int)height / _scale) / 2);
	_screenChangeID++;
	_cursorStale = true;
	dirtyAll();
}

void ChumbyGraphicsManager::setPalette(const byte *colors, uint start, uint num) {
	assert(start + num <= 256);
	memcpy(_palette + start * 3, colors, num * 3);
	for (uint i = start; i < start + num; i++) {
		const byte *c = _palette + i * 3;
		_pal565[i] = rgb565(c[0], c[1], c[2]);
		_palSpread[i] = spread(_pal565[i]);
	}
	if (_format.isCLUT8())
		dirtyAll();
	if (!_cursorPaletteEnabled)
		_cursorStale = true;
}

void ChumbyGraphicsManager::grabPalette(byte *colors, uint start, uint num) const {
	assert(start + num <= 256);
	memcpy(colors, _palette + start * 3, num * 3);
}

void ChumbyGraphicsManager::copyRectToScreen(const void *buf, int pitch, int x, int y, int w, int h) {
	Common::Rect r(x, y, x + w, y + h);
	r.clip(Common::Rect(_game.w, _game.h));
	if (r.isEmpty())
		return;
	const byte *src = (const byte *)buf + (r.top - y) * pitch + (r.left - x) * _format.bytesPerPixel;
	_game.copyRectToSurface(src, pitch, r.left, r.top, r.width(), r.height());
	dirtyGame(r);
}

Graphics::Surface *ChumbyGraphicsManager::lockScreen() {
	return &_game;
}

void ChumbyGraphicsManager::unlockScreen() {
	dirtyAll();
}

void ChumbyGraphicsManager::fillScreen(uint32 col) {
	_game.fillRect(Common::Rect(_game.w, _game.h), col);
	dirtyAll();
}

void ChumbyGraphicsManager::fillScreen(const Common::Rect &r, uint32 col) {
	Common::Rect c = r;
	c.clip(Common::Rect(_game.w, _game.h));
	if (c.isEmpty())
		return;
	_game.fillRect(c, col);
	dirtyGame(c);
}

void ChumbyGraphicsManager::setShakePos(int shakeXOffset, int shakeYOffset) {
	// Only vertical shakes are drawn, as the SCUMM games use them.
	if (shakeYOffset != _shakeY) {
		_shakeY = shakeYOffset;
		dirtyAll();
	}
}

void ChumbyGraphicsManager::dirtyGame(const Common::Rect &r) {
	if (_overlayVisible)
		return;
	const int s = _scale;
	dirtyScreen(Common::Rect(r.left / s + _xoff, (r.top + _shakeY) / s + _yoff,
	                         (r.right + s - 1) / s + _xoff, (r.bottom + _shakeY + s - 1) / s + _yoff));
}

void ChumbyGraphicsManager::dirtyScreen(const Common::Rect &r) {
	if (_allDirty)
		return;
	Common::Rect c = r;
	c.clip(Common::Rect(NC_W, NC_H));
	if (c.isEmpty())
		return;
	for (uint i = 0; i < _dirty.size(); i++) {
		if (_dirty[i].contains(c))
			return;
		if (c.contains(_dirty[i])) {
			_dirty[i] = c;
			return;
		}
	}
	if (_dirty.size() >= kMaxDirty)
		_allDirty = true;
	else
		_dirty.push_back(c);
}

/* Scales the game's part of the screen rectangle r (screen pixels) into
 * dst, an NC_W x NC_H RGB565 picture; r must lie inside the scaled game. */
static void scaleRows(uint16 *dst, const Graphics::Surface &game, const Common::Rect &r, int s, int xoff, int yoff,
                      int shakeY, const uint16 *pal565, const uint32 *palSpread) {
	const Graphics::PixelFormat &f = game.format;
	const bool is555 = f == kFormat555;
	for (int sy = r.top; sy < r.bottom; sy++) {
		uint16 *d = dst + sy * NC_W + r.left;
		int n = r.width();
		const int gy = (sy - yoff) * s - shakeY;
		if (gy < 0 || gy + s > game.h) {
			memset(d, 0, n * 2);
			continue;
		}
		const int gx = (r.left - xoff) * s;
		if (f.isCLUT8()) {
			const byte *a = (const byte *)game.getBasePtr(gx, gy);
			if (s == 1) {
				while (n--)
					*d++ = pal565[*a++];
				continue;
			}
			// The hot loop: two output pixels to each 32-bit store when the
			// destination allows it (it does for a full-width 640x480 game).
			const byte *b = a + game.pitch;
			if (((uintptr_t)d & 3) && n) {
				*d++ = fold4(palSpread[a[0]] + palSpread[a[1]] + palSpread[b[0]] + palSpread[b[1]]);
				a += 2, b += 2, n--;
			}
			uint32 *d32 = (uint32 *)d;
			for (; n >= 2; n -= 2) {
				const uint16 p0 = fold4(palSpread[a[0]] + palSpread[a[1]] + palSpread[b[0]] + palSpread[b[1]]);
				const uint16 p1 = fold4(palSpread[a[2]] + palSpread[a[3]] + palSpread[b[2]] + palSpread[b[3]]);
				*d32++ = p0 | ((uint32)p1 << 16);  // little-endian: the left pixel is the low half
				a += 4, b += 4;
			}
			d = (uint16 *)d32;
			if (n)
				*d = fold4(palSpread[a[0]] + palSpread[a[1]] + palSpread[b[0]] + palSpread[b[1]]);
		} else if (f.bytesPerPixel == 2) {
			const uint16 *a = (const uint16 *)game.getBasePtr(gx, gy);
			if (s == 1) {
				if (is555) {
					while (n--)
						*d++ = from555(*a++);
				} else {
					memcpy(d, a, n * 2);
				}
				continue;
			}
			const uint16 *b = (const uint16 *)((const byte *)a + game.pitch);
			if (is555) {
				for (; n; n--, a += 2, b += 2)
					*d++ = fold4(spread(from555(a[0])) + spread(from555(a[1])) + spread(from555(b[0])) +
					             spread(from555(b[1])));
			} else {
				for (; n; n--, a += 2, b += 2)
					*d++ = fold4(spread(a[0]) + spread(a[1]) + spread(b[0]) + spread(b[1]));
			}
		}
	}
}

void ChumbyGraphicsManager::scaleGame(const Common::Rect &r) {
	scaleRows(nc_screen(), _game, r, _scale, _xoff, _yoff, _shakeY, _pal565, _palSpread);
}

/* Draws screen rectangle r from what is behind it: the overlay, or the
 * scaled game and the black around it. */
void ChumbyGraphicsManager::redraw(const Common::Rect &r) {
	uint16 *screen = nc_screen();
	if (_overlayVisible) {
		for (int y = r.top; y < r.bottom; y++)
			memcpy(screen + y * NC_W + r.left, _overlay.getBasePtr(r.left, y), r.width() * 2);
		return;
	}
	Common::Rect g;
	if (_game.getPixels())
		g = Common::Rect(_xoff, _yoff, MIN<int>(NC_W, _xoff + _game.w / _scale),
		                 MIN<int>(NC_H, _yoff + _game.h / _scale));
	Common::Rect in = r.findIntersectingRect(g);
	if (in.isEmpty()) {
		for (int y = r.top; y < r.bottom; y++)
			memset(screen + y * NC_W + r.left, 0, r.width() * 2);
		return;
	}
	// The black around the game: whole rows above and below, then the
	// sides of the rows it spans.
	for (int y = r.top; y < r.bottom; y++) {
		if (y < in.top || y >= in.bottom) {
			memset(screen + y * NC_W + r.left, 0, r.width() * 2);
		} else {
			if (r.left < in.left)
				memset(screen + y * NC_W + r.left, 0, (in.left - r.left) * 2);
			if (in.right < r.right)
				memset(screen + y * NC_W + in.right, 0, (r.right - in.right) * 2);
		}
	}
	scaleGame(in);
}

void ChumbyGraphicsManager::updateScreen() {
	if (_cursorStale && _cursorVisible)
		rebuildCursor();
	Common::Rect next = _cursorVisible ? cursorRect() : Common::Rect();
	bool cursor = _cursorMoved || _allDirty;
	for (uint i = 0; !cursor && i < _dirty.size(); i++)
		cursor = _dirty[i].intersects(next) || _dirty[i].intersects(_cursorDrawn);
	// What the cursor covered is redrawn from what is behind it.
	if (cursor && !_cursorDrawn.isEmpty())
		dirtyScreen(_cursorDrawn);

	Common::Rect box;
	if (_allDirty) {
		box = Common::Rect(NC_W, NC_H);
		redraw(box);
	} else {
		for (uint i = 0; i < _dirty.size(); i++) {
			redraw(_dirty[i]);
			box.extend(_dirty[i]);
		}
	}
	_dirty.clear();
	_allDirty = false;
	if (cursor) {
		drawCursor();
		box.extend(_cursorDrawn);
	}
	_cursorMoved = false;
	if (!box.isEmpty())
		nc_present(box.left, box.top, box.width(), box.height());
}

void ChumbyGraphicsManager::showOverlay(bool inGUI) {
	if (_overlayVisible)
		return;
	_overlayVisible = true;
	_cursorStale = true;
	dirtyAll();
}

void ChumbyGraphicsManager::hideOverlay() {
	if (!_overlayVisible)
		return;
	_overlayVisible = false;
	_cursorStale = true;
	dirtyAll();
}

void ChumbyGraphicsManager::clearOverlay() {
	// The GUI draws over the game as it is on the screen.
	uint16 *dst = (uint16 *)_overlay.getPixels();
	memset(dst, 0, NC_W * NC_H * 2);
	if (!_game.getPixels())
		return;
	Common::Rect g(_xoff, _yoff, MIN<int>(NC_W, _xoff + _game.w / _scale), MIN<int>(NC_H, _yoff + _game.h / _scale));
	scaleRows(dst, _game, g, _scale, _xoff, _yoff, _shakeY, _pal565, _palSpread);
	if (_overlayVisible)
		dirtyAll();
}

void ChumbyGraphicsManager::grabOverlay(Graphics::Surface &surface) const {
	assert(surface.w >= _overlay.w && surface.h >= _overlay.h);
	assert(surface.format.bytesPerPixel == _overlay.format.bytesPerPixel);
	surface.copyRectToSurface(_overlay, 0, 0, Common::Rect(_overlay.w, _overlay.h));
}

void ChumbyGraphicsManager::copyRectToOverlay(const void *buf, int pitch, int x, int y, int w, int h) {
	Common::Rect r(x, y, x + w, y + h);
	r.clip(Common::Rect(_overlay.w, _overlay.h));
	if (r.isEmpty())
		return;
	const byte *src = (const byte *)buf + (r.top - y) * pitch + (r.left - x) * 2;
	_overlay.copyRectToSurface(src, pitch, r.left, r.top, r.width(), r.height());
	if (_overlayVisible)
		dirtyScreen(r);
}

bool ChumbyGraphicsManager::showMouse(bool visible) {
	bool was = _cursorVisible;
	if (visible != was) {
		_cursorVisible = visible;
		_cursorMoved = true;
	}
	return was;
}

void ChumbyGraphicsManager::warpMouse(int x, int y) {
	setMousePosition(x, y);
}

void ChumbyGraphicsManager::setMousePosition(int x, int y) {
	if (x != _mouseX || y != _mouseY) {
		_mouseX = x;
		_mouseY = y;
		_cursorMoved = true;
	}
}

Common::Point ChumbyGraphicsManager::screenToVirtual(int sx, int sy) const {
	if (_overlayVisible)
		return Common::Point(CLIP<int>(sx, 0, _overlay.w - 1), CLIP<int>(sy, 0, _overlay.h - 1));
	if (!_game.getPixels())
		return Common::Point(0, 0);
	return Common::Point(CLIP<int>((sx - _xoff) * _scale, 0, _game.w - 1),
	                     CLIP<int>((sy - _yoff) * _scale - _shakeY, 0, _game.h - 1));
}

void ChumbyGraphicsManager::setMouseCursor(const void *buf, uint w, uint h, int hotspotX, int hotspotY, uint32 keycolor,
                                           const Graphics::PixelFormat *format, const byte *mask, frac_t scaleX,
                                           frac_t scaleY) {
	Graphics::PixelFormat f = format ? *format : Graphics::PixelFormat::createFormatCLUT8();
	_cursorW = w;
	_cursorH = h;
	_cursorHotX = hotspotX;
	_cursorHotY = hotspotY;
	_cursorKey = keycolor;
	_cursorFormat = f;
	_cursorSrc.resize(w * h * f.bytesPerPixel);
	if (w * h)
		memcpy(&_cursorSrc[0], buf, w * h * f.bytesPerPixel);
	_cursorMask.clear();
	if (mask && w * h) {
		_cursorMask.resize(w * h);
		memcpy(&_cursorMask[0], mask, w * h);
	}
	_cursorStale = true;
	_cursorMoved = true;
}

void ChumbyGraphicsManager::setCursorPalette(const byte *colors, uint start, uint num) {
	assert(start + num <= 256);
	memcpy(_cursorPalette + start * 3, colors, num * 3);
	_cursorPaletteEnabled = true;
	_cursorStale = true;
}

/* The cursor as drawn: scaled like what it points at (halved over a halved
 * game, as it is over the overlay), in RGB565 with an opaque/transparent
 * mask. Rebuilt only when it, its palette or the scale changes. */
void ChumbyGraphicsManager::rebuildCursor() {
	_cursorStale = false;
	_cursorMoved = true;
	const int s = _overlayVisible ? 1 : _scale;
	_cursorScale = s;
	_cursorDrawW = (_cursorW + s - 1) / s;
	_cursorDrawH = (_cursorH + s - 1) / s;
	_cursorDrawHotX = _cursorHotX / s;
	_cursorDrawHotY = _cursorHotY / s;
	_cursorPixels.resize(_cursorDrawW * _cursorDrawH);
	_cursorAlpha.resize(_cursorDrawW * _cursorDrawH);
	if (!_cursorDrawW || !_cursorDrawH)
		return;

	const byte *pal = _cursorPaletteEnabled ? _cursorPalette : _palette;
	const Graphics::PixelFormat &f = _cursorFormat;
	// One source pixel: whether it shows, and its colour.
	auto pixel = [&](uint x, uint y, byte &r, byte &g, byte &b) -> bool {
		const uint i = y * _cursorW + x;
		uint32 c;
		if (f.bytesPerPixel == 1)
			c = _cursorSrc[i];
		else if (f.bytesPerPixel == 2)
			c = READ_UINT16(&_cursorSrc[i * 2]);
		else
			c = READ_UINT32(&_cursorSrc[i * 4]);
		if (!_cursorMask.empty()) {
			if (_cursorMask[i] != kCursorMaskOpaque)
				return false;
		} else if (c == _cursorKey) {
			return false;
		}
		if (f.isCLUT8()) {
			r = pal[c * 3], g = pal[c * 3 + 1], b = pal[c * 3 + 2];
		} else {
			f.colorToRGB(c, r, g, b);
		}
		return true;
	};
	for (int y = 0; y < _cursorDrawH; y++) {
		for (int x = 0; x < _cursorDrawW; x++) {
			uint sr = 0, sg = 0, sb = 0, n = 0;
			for (int dy = 0; dy < s; dy++) {
				for (int dx = 0; dx < s; dx++) {
					const uint px = x * s + dx, py = y * s + dy;
					byte r, g, b;
					if (px < _cursorW && py < _cursorH && pixel(px, py, r, g, b))
						sr += r, sg += g, sb += b, n++;
				}
			}
			const int i = y * _cursorDrawW + x;
			_cursorAlpha[i] = n >= (s == 1 ? 1u : (uint)kCursorOpaqueOfFour);
			_cursorPixels[i] = n ? rgb565(sr / n, sg / n, sb / n) : 0;
		}
	}
}

Common::Rect ChumbyGraphicsManager::cursorRect() const {
	if (!_cursorDrawW || !_cursorDrawH)
		return Common::Rect();
	int sx, sy;
	if (_overlayVisible) {
		sx = _mouseX, sy = _mouseY;
	} else {
		sx = _mouseX / _scale + _xoff;
		sy = (_mouseY + _shakeY) / _scale + _yoff;
	}
	Common::Rect r(sx - _cursorDrawHotX, sy - _cursorDrawHotY, sx - _cursorDrawHotX + _cursorDrawW,
	               sy - _cursorDrawHotY + _cursorDrawH);
	r.clip(Common::Rect(NC_W, NC_H));
	return r;
}

void ChumbyGraphicsManager::drawCursor() {
	_cursorDrawn = Common::Rect();
	// Recordings (the host test mode's) are taken without it.
	static const bool hidden = getenv("NEOCHUMBY_SCUMMVM_NO_CURSOR") != nullptr;
	if (!_cursorVisible || _cursorStale || hidden)
		return;
	const Common::Rect r = cursorRect();
	if (r.isEmpty())
		return;
	// The cursor's own top left, which may be off the screen.
	int sx, sy;
	if (_overlayVisible) {
		sx = _mouseX, sy = _mouseY;
	} else {
		sx = _mouseX / _scale + _xoff;
		sy = (_mouseY + _shakeY) / _scale + _yoff;
	}
	const int cx0 = sx - _cursorDrawHotX, cy0 = sy - _cursorDrawHotY;
	uint16 *screen = nc_screen();
	for (int y = r.top; y < r.bottom; y++) {
		const int ci = (y - cy0) * _cursorDrawW;
		for (int x = r.left; x < r.right; x++) {
			if (_cursorAlpha[ci + x - cx0])
				screen[y * NC_W + x] = _cursorPixels[ci + x - cx0];
		}
	}
	_cursorDrawn = r;
}
