// ambience_bridge.h — the scene-graph half of AmbienceItem (ambience_item.rs, docs/AMBIENCE.md): ONE geometry node that
// draws the shared mesh (tinhat_widgets::ambience — triangles, x y r g b a per vertex, colours premultiplied) with
// Qt's vertex-colour material. No custom shader, so it draws the same on every RHI backend (D3D11, Vulkan, OpenGL,
// Metal) and needs no .qsb step. (Qt's software renderer can't draw it: the QML turns effects off there.)
// Header-only (templates): the cxx-qt bridge instantiates them with the generated AmbienceItem class.
// a macro guard, not #pragma once: the build sees this file under two paths (src/ and cxx-qt's include copy)
#ifndef TINHAT_AMBIENCE_BRIDGE_H
#define TINHAT_AMBIENCE_BRIDGE_H

#include <cstddef>
#include <QtQuick/QQuickItem>
#include <QtQuick/QQuickWindow>
#include <QtQuick/QSGFlatColorMaterial>
#include <QtQuick/QSGGeometry>
#include <QtQuick/QSGGeometryNode>
#include <QtQuick/QSGNode>
#include <QtQuick/QSGVertexColorMaterial>

// cxx can't name a nested class: the override's parameter type under a plain name.
using QQuickItemUpdatePaintNodeData = QQuickItem::UpdatePaintNodeData;

// The item draws (QQuickItem::ItemHasContents) — set once from its Initialize.
template <class T>
void tinhatAmbienceInit(T &item) {
    item.setFlag(QQuickItem::ItemHasContents, true);
}

// Is this window drawn by a CPU rasteriser (llvmpipe / softpipe / SwiftShader / WARP)? Then every animated frame
// redraws the whole window on the CPU: the QML keeps the still background and runs no particles.
extern "C" bool tinhat_ambience_cpu_device(QQuickWindow *w); // ambience_image.cpp (where Qt's RHI header is reachable)
template <class T>
bool tinhatAmbienceCpuDevice(T &item) {
    return tinhat_ambience_cpu_device(item.window());
}

namespace tinhat_ambience_detail {
inline unsigned char u8(float c) {
    if (!(c > 0.f)) return 0;
    if (c >= 1.f) return 255;
    return static_cast<unsigned char>(c * 255.f + 0.5f);
}
} // namespace tinhat_ambience_detail

// updatePaintNode's body: `v` holds `nFloats` floats (6 per vertex). No vertices → no node (the old one is deleted, as
// QQuickItem::updatePaintNode documents). Runs on the render thread while the GUI thread is blocked.
template <class T>
QSGNode *tinhatAmbienceNode(T &, QSGNode *old, const float *v, std::size_t nFloats) {
    const int verts = static_cast<int>(nFloats / 6);
    auto *node = static_cast<QSGGeometryNode *>(old);
    if (verts <= 0 || !v) {
        delete node;
        return nullptr;
    }
    if (!node) {
        node = new QSGGeometryNode;
        auto *g = new QSGGeometry(QSGGeometry::defaultAttributes_ColoredPoint2D(), verts);
        g->setDrawingMode(QSGGeometry::DrawTriangles);
        g->setVertexDataPattern(QSGGeometry::StreamPattern);
        node->setGeometry(g);
        node->setFlag(QSGNode::OwnsGeometry);
        node->setMaterial(new QSGVertexColorMaterial);
        node->setFlag(QSGNode::OwnsMaterial);
    }
    QSGGeometry *g = node->geometry();
    if (g->vertexCount() != verts) g->allocate(verts);
    QSGGeometry::ColoredPoint2D *p = g->vertexDataAsColoredPoint2D();
    using tinhat_ambience_detail::u8;
    for (int i = 0; i < verts; ++i) {
        const float *q = v + static_cast<std::size_t>(i) * 6;
        p[i].set(q[0], q[1], u8(q[2]), u8(q[3]), u8(q[4]), u8(q[5]));
    }
    g->markVertexDataDirty();
    node->markDirty(QSGNode::DirtyGeometry);
    return node;
}

#endif // TINHAT_AMBIENCE_BRIDGE_H
