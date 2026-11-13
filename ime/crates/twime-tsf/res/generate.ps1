# Self-drawn assets; Noto Sans TC (SIL OFL 1.1), no downloaded inputs.
# Run with Windows PowerShell: powershell -File res/generate.ps1
#
# Black and white, after Microsoft's IME icon guideline and the built-in Traditional Chinese IME (IMTCTIP.dll), user
# decisions 2026-10-27 (https://learn.microsoft.com/en-us/windows/apps/develop/input/input-method-editor-requirements).
# Icon groups (resource type 14):
#   101 tip.ico            brand icon (RegisterProfile index 0): black U+6CE8 (Regular) on a square box filled #F3F3F3
#                          (Windows 11 light base color), no stroke: transparent-looking on a light taskbar, a light
#                          box on a dark one
#   102 chinese-light.ico  mode button for a light taskbar: plain black U+4E2D (Regular)
#   103 english-light.ico  mode button for a light taskbar: plain black U+82F1
#   104 chinese-dark.ico   mode button for a dark taskbar: white U+4E2D (Regular), 1 px outer stroke black at 50% opacity
#   105 english-dark.ico   mode button for a dark taskbar: white U+82F1, same style
# langbar.rs picks 102/103 or 104/105 from SystemUsesLightTheme.
# Each group has 8 32-bit PNG images with alpha, 16 to 256 px; image ids 1 + group * 8 + size index.
$ErrorActionPreference = 'Stop'
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.IO;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.Drawing.Text;
public static class TwimeIcons {
    static void Record(BinaryWriter w, ushort type, ushort id, byte[] bytes) {
        w.Write((uint)bytes.Length); w.Write((uint)32);
        w.Write((ushort)0xffff); w.Write(type); w.Write((ushort)0xffff); w.Write(id);
        w.Write((uint)0); w.Write((ushort)0x1030); w.Write((ushort)0);
        w.Write((uint)0); w.Write((uint)0); w.Write(bytes);
        while (w.BaseStream.Position % 4 != 0) w.Write((byte)0);
    }
    // Glyph outline centered in the n x n icon, scaled so its larger side is `extent` pixels.
    static GraphicsPath Glyph(FontFamily family, string glyph, int n, float extent) {
        var text = new GraphicsPath();
        // Winding: the variable font draws overlapping strokes; alternate filling would punch holes.
        text.FillMode = FillMode.Winding;
        text.AddString(glyph, family, (int)FontStyle.Regular, n, new PointF(0,0), StringFormat.GenericTypographic);
        var b = text.GetBounds();
        float fit = Math.Min(extent/b.Width, extent/b.Height);
        using (var m = new Matrix()) {
            m.Translate(n/2f, n/2f); m.Scale(fit, fit); m.Translate(-b.X-b.Width/2, -b.Y-b.Height/2);
            text.Transform(m);
        }
        return text;
    }
    public static void Generate(string dir) {
        int[] sizes = {16,20,24,32,40,48,64,256};
        string[] names = {"tip", "chinese-light", "english-light", "chinese-dark", "english-dark"};
        string[] glyphs = {"\u6ce8", "\u4e2d", "\u82f1", "\u4e2d", "\u82f1"};
        var half = Color.FromArgb(128, 0, 0, 0);   // black at 50% opacity
        var boxFill = ColorTranslator.FromHtml("#F3F3F3");
        using (var fonts = new PrivateFontCollection())
        using (var res = new BinaryWriter(File.Create(Path.Combine(dir,"twime-icons.res")))) {
            fonts.AddFontFile(@"C:\Windows\Fonts\NotoSansTC-VF.ttf");
            Record(res, 0, 0, new byte[0]); // mandatory null resource header
            for (int g=0; g<names.Length; g++) {
                var images = new byte[sizes.Length][];
                for (int i=0; i<sizes.Length; i++) {
                    int n=sizes[i], scale=4;
                    using(var large=new Bitmap(n*scale,n*scale,PixelFormat.Format32bppArgb))
                    using(var canvas=Graphics.FromImage(large)) {
                        canvas.ScaleTransform(scale,scale);   // drawing units are final pixels
                        canvas.SmoothingMode=SmoothingMode.AntiAlias;
                        if (g == 0) {
                            // Only the glyph is supersampled (15% margins); the box is set pixel-exact after downscaling.
                            using (var text = Glyph(fonts.Families[0], glyphs[g], n, n*0.70f)) canvas.FillPath(Brushes.Black, text);
                        } else if (g <= 2) {
                            using (var text = Glyph(fonts.Families[0], glyphs[g], n, n-2)) canvas.FillPath(Brushes.Black, text);
                        } else {
                            // White glyph; a 2 px pen centered on the outline leaves a 1 px stroke outside the fill.
                            using (var text = Glyph(fonts.Families[0], glyphs[g], n, n-4)) {
                                using (var pen = new Pen(half, 2f) { LineJoin = LineJoin.Round }) canvas.DrawPath(pen, text);
                                canvas.FillPath(Brushes.White, text);
                            }
                        }
                        using(var small=new Bitmap(n,n,PixelFormat.Format32bppArgb))
                        using(var down=Graphics.FromImage(small))
                        using(var png=new MemoryStream()) {
                            down.InterpolationMode=InterpolationMode.HighQualityBicubic;
                            down.PixelOffsetMode=PixelOffsetMode.HighQuality;
                            down.DrawImage(large,new Rectangle(0,0,n,n));
                            if (g == 0) {
                                // Opaque #F3F3F3 square over the whole icon, glyph composited on top.
                                using (var box = new Bitmap(n,n,PixelFormat.Format32bppArgb)) {
                                    for (int y=0; y<n; y++) for (int x=0; x<n; x++) box.SetPixel(x, y, boxFill);
                                    using (var over = Graphics.FromImage(box)) over.DrawImage(small, new Rectangle(0,0,n,n));
                                    box.Save(png,ImageFormat.Png);
                                }
                            } else {
                                small.Save(png,ImageFormat.Png);
                            }
                            images[i]=png.ToArray();
                        }
                    }
                    Record(res,3,(ushort)(1+g*8+i),images[i]);
                }
                using(var group=new MemoryStream())
                using(var gw=new BinaryWriter(group))
                using(var ico=new BinaryWriter(File.Create(Path.Combine(dir,names[g]+".ico")))) {
                    gw.Write((ushort)0); gw.Write((ushort)1); gw.Write((ushort)8);
                    ico.Write((ushort)0); ico.Write((ushort)1); ico.Write((ushort)8);
                    uint offset=6+16*8;
                    for(int i=0;i<8;i++) {
                        foreach(var w in new[]{gw,ico}) {
                            w.Write((byte)(sizes[i]%256)); w.Write((byte)(sizes[i]%256));
                            w.Write((byte)0); w.Write((byte)0); w.Write((ushort)1); w.Write((ushort)32); w.Write((uint)images[i].Length);
                        }
                        gw.Write((ushort)(1+g*8+i)); ico.Write(offset); offset+=(uint)images[i].Length;
                    }
                    foreach(var bytes in images) ico.Write(bytes);
                    Record(res,14,(ushort)(101+g),group.ToArray());
                }
            }
        }
    }
}
'@
[TwimeIcons]::Generate($PSScriptRoot)
