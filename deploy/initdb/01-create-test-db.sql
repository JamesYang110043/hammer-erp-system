-- 第一次啟動（volume 為空）時由 postgres 映像檔自動執行。
-- 開發用資料庫 hammer_erp 由環境變數 POSTGRES_DB 建立；這裡另外建立測試用資料庫。
-- 擁有者為執行此腳本的使用者（POSTGRES_USER）。
CREATE DATABASE hammer_erp_test;
