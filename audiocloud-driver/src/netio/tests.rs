use trim_margin::MarginTrimmable;

use crate::netio::power_pdu_4c::NetioPowerResponse;

#[test]
fn deserialize_captured_response() {
    let raw_json = r#"|{
                      |	"Agent":	{
                      |		"Model":	"PowerPDU 4C",
                      |		"Version":	"3.4.2",
                      |		"JSONVer":	"2.1",
                      |		"DeviceName":	"rack1-up-right",
                      |		"VendorID":	0,
                      |		"OemID":	0,
                      |		"SerialNumber":	"24:A4:2C:39:3F:05",
                      |		"Uptime":	4521231,
                      |		"Time":	"2022-10-11T15:11:46+01:00",
                      |		"NumOutputs":	4
                      |	},
                      |	"GlobalMeasure":	{
                      |		"Voltage":	222.4,
                      |		"Frequency":	50.0,
                      |		"TotalCurrent":	0,
                      |		"OverallPowerFactor":	0.00,
                      |		"TotalLoad":	0,
                      |		"TotalEnergy":	233220,
                      |		"EnergyStart":	"1970-01-01T01:00:00+01:00"
                      |	},
                      |	"Outputs":	[{
                      |			"ID":	1,
                      |			"Name":	"output_1",
                      |			"State":	1,
                      |			"Action":	6,
                      |			"Delay":	5000,
                      |			"Current":	0,
                      |			"PowerFactor":	0.00,
                      |			"Load":	0,
                      |			"Energy":	233203
                      |		}, {
                      |			"ID":	2,
                      |			"Name":	"mac_mini",
                      |			"State":	1,
                      |			"Action":	6,
                      |			"Delay":	5000,
                      |			"Current":	0,
                      |			"PowerFactor":	0.00,
                      |			"Load":	0,
                      |			"Energy":	0
                      |		}, {
                      |			"ID":	3,
                      |			"Name":	"output_3",
                      |			"State":	1,
                      |			"Action":	6,
                      |			"Delay":	5000,
                      |			"Current":	0,
                      |			"PowerFactor":	0.00,
                      |			"Load":	0,
                      |			"Energy":	8
                      |		}, {
                      |			"ID":	4,
                      |			"Name":	"output_4",
                      |			"State":	1,
                      |			"Action":	6,
                      |			"Delay":	5000,
                      |			"Current":	0,
                      |			"PowerFactor":	0.00,
                      |			"Load":	0,
                      |			"Energy":	8
                      |		}]
                      |}"#.trim_margin()
                          .expect("Failed to trim margin from captured JSON");

    let netio_response =
        serde_json::from_str::<NetioPowerResponse>(raw_json.as_str()).expect("Captured response should deserialize");

    // TODO: assert values
}

#[test]
fn serialize_request() {
    // TODO: add test when implementation exists
}
