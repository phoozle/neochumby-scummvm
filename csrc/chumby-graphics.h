/* neochumby-scummvm: ScummVM's graphics on the Chumby's 320x240 RGB565
 * screen.
 *
 * Copyright (C) 2026 the neochumby-scummvm authors
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version. See LICENSE.
 */

#ifndef NEOCHUMBY_SCUMMVM_GRAPHICS_H
#define NEOCHUMBY_SCUMMVM_GRAPHICS_H

#include "backends/graphics/graphics.h"
#include "common/array.h"
#include "common/rect.h"
#include "graphics/surface.h"

/**
 * The game's screen is kept at its own size (640x480 for the HE games) and
 * scaled into the Chumby's screen at updateScreen(): halved, each output
 * pixel the average of a 2x2 block, when the game is bigger than 320x240,
 * else copied, centred. Only what changed since the last update is scaled
 * (the dirty rectangles), and the cursor is drawn on the scaled picture.
 *
 * The overlay (ScummVM's own menus) is 320x240 RGB565, the screen's own
 * size and format, so the GUI draws at the screen's resolution and is
 * copied as it is.
 *
 * The screen mode is never changed (docs/hardware.md in neochumby: doing
 * so breaks the LCD controller), and palettes are applied in software.
 */
class ChumbyGraphicsManager : public GraphicsManager {
public:
	ChumbyGraphicsManager();
	~ChumbyGraphicsManager() override;

	bool hasFeature(OSystem::Feature f) const override;
	void setFeatureState(OSystem::Feature f, bool enable) override;
	bool getFeatureState(OSystem::Feature f) const override;

#ifdef USE_RGB_COLOR
	Graphics::PixelFormat getScreenFormat() const override { return _format; }
	Common::List<Graphics::PixelFormat> getSupportedFormats() const override;
#endif
	void initSize(uint width, uint height, const Graphics::PixelFormat *format = nullptr) override;
	int getScreenChangeID() const override { return _screenChangeID; }

	void beginGFXTransaction() override {}
	OSystem::TransactionError endGFXTransaction() override { return _transactionError; }

	int16 getHeight() const override { return _game.h; }
	int16 getWidth() const override { return _game.w; }
	void setPalette(const byte *colors, uint start, uint num) override;
	void grabPalette(byte *colors, uint start, uint num) const override;
	void copyRectToScreen(const void *buf, int pitch, int x, int y, int w, int h) override;
	Graphics::Surface *lockScreen() override;
	void unlockScreen() override;
	void fillScreen(uint32 col) override;
	void fillScreen(const Common::Rect &r, uint32 col) override;
	void updateScreen() override;
	void setShakePos(int shakeXOffset, int shakeYOffset) override;
	void setFocusRectangle(const Common::Rect &rect) override {}
	void clearFocusRectangle() override {}

	void showOverlay(bool inGUI) override;
	void hideOverlay() override;
	bool isOverlayVisible() const override { return _overlayVisible; }
	Graphics::PixelFormat getOverlayFormat() const override { return _overlay.format; }
	void clearOverlay() override;
	void grabOverlay(Graphics::Surface &surface) const override;
	void copyRectToOverlay(const void *buf, int pitch, int x, int y, int w, int h) override;
	int16 getOverlayHeight() const override { return _overlay.h; }
	int16 getOverlayWidth() const override { return _overlay.w; }

	bool showMouse(bool visible) override;
	void warpMouse(int x, int y) override;
	void setMouseCursor(const void *buf, uint w, uint h, int hotspotX, int hotspotY, uint32 keycolor,
	                    const Graphics::PixelFormat *format, const byte *mask, frac_t scaleX, frac_t scaleY) override;
	void setCursorPalette(const byte *colors, uint start, uint num) override;

	/** A point on the Chumby's screen, in the coordinates ScummVM's events
	 * use now: the overlay's when it shows, else the game's. */
	Common::Point screenToVirtual(int sx, int sy) const;
	/** The mouse moved to (x, y), in those coordinates. */
	void setMousePosition(int x, int y);

private:
	void dirtyGame(const Common::Rect &r);
	void dirtyScreen(const Common::Rect &r);
	void dirtyAll() { _allDirty = true; }
	void redraw(const Common::Rect &r);
	void scaleGame(const Common::Rect &r);
	void rebuildCursor();
	Common::Rect cursorRect() const;
	void drawCursor();

	Graphics::Surface _game;
	Graphics::PixelFormat _format;
	Graphics::Surface _overlay;
	bool _overlayVisible;
	int _screenChangeID;
	OSystem::TransactionError _transactionError;

	/** Game pixels per screen pixel (1 or 2), and the scaled game's place. */
	int _scale;
	int _xoff, _yoff;
	int _shakeY;

	byte _palette[256 * 3];
	/** The palette in RGB565, and spread (see chumby-graphics.cpp). */
	uint16 _pal565[256];
	uint32 _palSpread[256];

	/** Changed parts of the screen, in screen pixels. */
	Common::Array<Common::Rect> _dirty;
	bool _allDirty;

	// The cursor as set (in game or overlay pixels), and as drawn.
	Common::Array<byte> _cursorSrc;
	Common::Array<byte> _cursorMask;
	uint _cursorW, _cursorH;
	int _cursorHotX, _cursorHotY;
	uint32 _cursorKey;
	Graphics::PixelFormat _cursorFormat;
	byte _cursorPalette[256 * 3];
	bool _cursorPaletteEnabled;
	bool _cursorStale;
	int _cursorScale;
	Common::Array<uint16> _cursorPixels;
	Common::Array<byte> _cursorAlpha;
	int _cursorDrawW, _cursorDrawH, _cursorDrawHotX, _cursorDrawHotY;
	bool _cursorVisible;
	int _mouseX, _mouseY;
	/** Where the cursor was last drawn, on the screen; empty if it wasn't. */
	Common::Rect _cursorDrawn;
	bool _cursorMoved;
};

#endif
