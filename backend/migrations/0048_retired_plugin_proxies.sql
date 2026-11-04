-- 停用的插件历史仅保留归档事实，不能阻止原生代理管理
ALTER TABLE plugin_update_sources
    DROP CONSTRAINT plugin_update_sources_outbound_proxy_id_fkey,
    ADD CONSTRAINT plugin_update_sources_outbound_proxy_id_fkey
        FOREIGN KEY (outbound_proxy_id) REFERENCES outbound_proxies(id) ON DELETE SET NULL;

ALTER TABLE plugin_artifacts
    DROP CONSTRAINT plugin_artifacts_outbound_proxy_id_fkey,
    ADD CONSTRAINT plugin_artifacts_outbound_proxy_id_fkey
        FOREIGN KEY (outbound_proxy_id) REFERENCES outbound_proxies(id) ON DELETE SET NULL;
