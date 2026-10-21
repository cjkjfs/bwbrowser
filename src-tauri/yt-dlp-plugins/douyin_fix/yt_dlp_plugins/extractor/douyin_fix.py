"""抖音提取器补丁（yt-dlp 插件）

官方的 DouyinIE 请求 `aweme/v1/web/aweme/detail/` 时缺少 `x-secsdk-web-signature`
签名，抖音服务端固定返回 403，报错文案却是 "Fresh cookies ... are needed"。

这里按上游 PR #17826 的做法补齐签名与 `aid=6383` 参数，并自行注册 ttwid，
因此不需要用户安装 Python，也不需要携带登录 cookie。

插件目录结构：<plugin-dirs>/douyin_fix/yt_dlp_plugins/extractor/douyin_fix.py
"""

import hashlib
import json
import re
import time
import urllib.parse
import uuid

from yt_dlp.extractor.common import InfoExtractor
from yt_dlp.extractor.tiktok import DouyinIE
from yt_dlp.utils import ExtractorError, traverse_obj


class DouyinFixIE(DouyinIE):
    _VALID_URL = r'https?://(?:www\.)?douyin\.com/video/(?P<id>[0-9]+)'
    _SEC_SALT = 'A96D855A08C0A9707F8BEF0D9A527E4E'
    _REQUIRED_COOKIES = ['passport_csrf_token', 'ttwid']
    _AID = 6383  # aweme_general，缺这个参数只能拿到 720p
    _CSRF = uuid.uuid4().hex

    def _get_ttwid(self):
        self._request_webpage(f'{self._WEBPAGE_HOST}ttwid/register', None, data=json.dumps({
            'aid': self._AID,
            'service': 'www.douyin.com',
        }).encode(), headers={'content-type': 'application/json'})
        cookie = self._get_cookies(self._WEBPAGE_HOST).get('ttwid')
        return cookie.value if cookie else None

    def check_and_get_required_cookies(self):
        for rc in self._REQUIRED_COOKIES:
            if self._get_cookies(self._WEBPAGE_HOST).get(rc):
                continue
            cookie = self._get_ttwid() if rc == 'ttwid' else self._CSRF
            if not cookie:
                raise ExtractorError('Unable to generate required cookies')
            self._set_cookie('.douyin.com', rc, cookie)

    def _get_uifid(self):
        names = ('uifid', 'uifid_temp', 'uifidtemp', 'UIFID', 'UIFID_TEMP', 'UIFIDTEMP')
        uifid = traverse_obj(self._get_cookies(self._WEBPAGE_HOST), (names, 'value'), get_all=False)
        if not uifid:
            urlh = self._request_webpage(self._WEBPAGE_HOST, None)
            return urlh.headers.get('x-tt-trace-host')
        return uifid

    def _sign_query(self, query):
        uifid = self._get_uifid()
        timestamp = str(int(time.time()))
        query = {**query, 'uifid': uifid, 'timestamp': timestamp}
        encoded_query = urllib.parse.urlencode(query)
        signature = hashlib.md5(
            f'{uifid}_{timestamp}_{self._SEC_SALT}_{encoded_query}'.encode()).hexdigest()
        return {**query, 'x-secsdk-web-signature': signature}

    def _real_extract(self, url):
        video_id = self._match_id(url)
        self.check_and_get_required_cookies()
        detail = traverse_obj(self._download_json(
            'https://www.douyin.com/aweme/v1/web/aweme/detail/', video_id,
            'Downloading web detail JSON', 'Failed to download web detail JSON',
            query=self._sign_query({'aweme_id': video_id, 'aid': self._AID}), fatal=False),
            ('aweme_detail', {dict}))
        if not detail:
            raise ExtractorError(
                'Fresh cookies (not necessarily logged in) are needed',
                expected=not self._get_cookies(self._WEBPAGE_HOST).get('s_v_web_id'))
        return {**self._parse_aweme_video_app(detail), 'http_headers': {'Referer': self._WEBPAGE_HOST}}


class DouyinShareFixIE(InfoExtractor):
    """iesdouyin 分享页：URL 里已经带视频 ID，直接转成标准视频页"""
    IE_NAME = 'douyin:share'
    _VALID_URL = r'https?://(?:www\.)?iesdouyin\.com/share/(?:video|note)/(?P<id>\d{8,30})'

    def _real_extract(self, url):
        return self.url_result(
            f'https://www.douyin.com/video/{self._match_id(url)}', DouyinFixIE.ie_key())


class DouyinShortFixIE(InfoExtractor):
    """v.douyin.com 短链：跟随跳转拿视频 ID"""
    IE_NAME = 'douyin:short'
    _VALID_URL = r'https?://v\.douyin\.com/(?P<id>[0-9A-Za-z_-]+)'

    def _real_extract(self, url):
        urlh = self._request_webpage(url, None, 'Resolving Douyin short link')
        final_url = urlh.url
        match = (re.search(r'/(?:video|note)/(\d{8,30})', final_url)
                 or re.search(r'[?&]modal_id=(\d{8,30})', final_url))
        if not match:
            raise ExtractorError('Unable to resolve Douyin short link', expected=True)
        return self.url_result(
            f'https://www.douyin.com/video/{match.group(1)}', DouyinFixIE.ie_key())
