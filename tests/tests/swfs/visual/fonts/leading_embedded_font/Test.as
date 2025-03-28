package {
import flash.display.Sprite;
import flash.text.TextField;
import flash.text.TextFormat;

[SWF(width="200", height="200")]
public class Test extends Sprite {
    [Embed(source="TestFontGap0.ttf", fontName="TestFontGap0", embedAsCFF="false", unicodeRange="U+0061-U+0064")]
    private var testFontGap0:Class;

    [Embed(source="TestFontGap100.ttf", fontName="TestFontGap100", embedAsCFF="false", unicodeRange="U+0061-U+0064")]
    private var testFontGap100:Class;

    private var nextX:int = 0;
    private var nextY:int = 0;

    public function Test() {
        stage.scaleMode = "noScale";
        addChild(newTextField("TestFontGap0"));
        addChild(newTextField("TestFontGap0", 0));
        addChild(newTextField("TestFontGap0", 2));
        nextY += 100;
        nextX = 0;
        addChild(newTextField("TestFontGap100"));
        addChild(newTextField("TestFontGap100", 0));
        addChild(newTextField("TestFontGap100", 2));
    }

    private function newTextField(font: String, leading:* = null): TextField {
        var text:TextField = new TextField();
        text.border = true;
        text.width = 50;
        text.x = nextX;
        text.embedFonts = true;
        var tf:TextFormat = new TextFormat();
        tf.font = font;
        tf.size = 20;
        if (leading != null) {
            tf.leading = leading;
        }
        text.defaultTextFormat = tf;

        text.y = nextY;
        text.height = 80;
        text.multiline = true;
        text.text = "acbd\nabcd";

        nextX += text.width + 2;
        return text;
    }

}
}
